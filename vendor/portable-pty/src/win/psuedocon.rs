use super::WinChild;
use crate::cmdbuilder::CommandBuilder;
use crate::win::procthreadattr::ProcThreadAttributeList;
use anyhow::{bail, ensure, Error};
use filedescriptor::{FileDescriptor, OwnedHandle};
use lazy_static::lazy_static;
use shared_library::shared_library;
use std::ffi::OsString;
use std::io::Error as IoError;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::{mem, ptr};
use winapi::shared::minwindef::DWORD;
use winapi::shared::winerror::{HRESULT, S_OK};
use winapi::um::handleapi::*;
use winapi::um::processthreadsapi::*;
use winapi::um::winbase::{
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use winapi::um::wincon::COORD;
use winapi::um::winnt::HANDLE;

pub type HPCON = HANDLE;

pub const PSUEDOCONSOLE_INHERIT_CURSOR: DWORD = 0x1;
pub const PSEUDOCONSOLE_RESIZE_QUIRK: DWORD = 0x2;
pub const PSEUDOCONSOLE_WIN32_INPUT_MODE: DWORD = 0x4;
#[allow(dead_code)]
pub const PSEUDOCONSOLE_PASSTHROUGH_MODE: DWORD = 0x8;

shared_library!(ConPtyFuncs,
    pub fn CreatePseudoConsole(
        size: COORD,
        hInput: HANDLE,
        hOutput: HANDLE,
        flags: DWORD,
        hpc: *mut HPCON
    ) -> HRESULT,
    pub fn ResizePseudoConsole(hpc: HPCON, size: COORD) -> HRESULT,
    pub fn ClosePseudoConsole(hpc: HPCON),
);

// Paneldeck patch (see PANELDECK-PATCH.md): upstream resolves `conpty.dll` once per process with a bare-name
// `LoadLibrary`, i.e. through the DLL search order (which may include the current directory), and uses it for every
// pseudo console. Here the caller chooses per `ConPtySystem` a sideloaded `conpty.dll` by full path or the system
// ConPTY (kernel32); a sideloaded DLL that cannot be loaded or fails to create the pseudo console falls back to kernel32.

lazy_static! {
    static ref KERNEL: Option<ConPtyFuncs> = ConPtyFuncs::open(Path::new("kernel32.dll")).ok();
    /// Loaded sideloaded DLLs by full path (never unloaded: pseudo consoles may still use their functions).
    static ref SIDELOADED: Mutex<Vec<(PathBuf, &'static ConPtyFuncs)>> = Mutex::new(Vec::new());
}

fn kernel_conpty() -> Result<&'static ConPtyFuncs, Error> {
    KERNEL.as_ref().ok_or_else(|| {
        anyhow::anyhow!("this system does not support conpty.  Windows 10 October 2018 or newer is required")
    })
}

/// Load `path` (an absolute path to a `conpty.dll`) once and keep it loaded.
fn sideloaded_conpty(path: &Path) -> Result<&'static ConPtyFuncs, Error> {
    ensure!(path.is_absolute(), "sideloaded conpty.dll must be given by absolute path: {}", path.display());
    let mut cache = SIDELOADED.lock().unwrap();
    if let Some((_, f)) = cache.iter().find(|(p, _)| p == path) {
        return Ok(*f);
    }
    let funcs = ConPtyFuncs::open(path).map_err(|e| anyhow::anyhow!("loading {}: {:?}", path.display(), e))?;
    let funcs: &'static ConPtyFuncs = Box::leak(Box::new(funcs));
    cache.push((path.to_path_buf(), funcs));
    Ok(funcs)
}

/// Which ConPTY implementation a pseudo console runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConPtyHost {
    /// The sideloaded `conpty.dll` (with its `OpenConsole.exe`).
    Sideloaded,
    /// The ConPTY built into Windows (kernel32, inbox conhost).
    System,
}

// Paneldeck patch: Job Object for the next `spawn_command` on this thread (see `set_spawn_job`).
thread_local! {
    static SPAWN_JOB: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static SPAWNED_IN_JOB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Paneldeck patch: start the process of the next `spawn_command` on this thread directly inside the Job Object
/// `job` (`PROC_THREAD_ATTRIBUTE_JOB_LIST`), so it is a member before its first instruction runs. Used once, then
/// cleared. If `CreateProcessW` refuses the job (e.g. a job of the caller forbids nesting), the process is started
/// without it and [`spawned_in_job`] returns `false`.
pub fn set_spawn_job(job: Option<std::os::windows::io::RawHandle>) {
    SPAWN_JOB.with(|j| j.set(job.map(|h| h as usize)));
    SPAWNED_IN_JOB.with(|s| s.set(false));
}

/// Paneldeck patch: did the last `spawn_command` on this thread start its process inside the job from
/// [`set_spawn_job`]?
pub fn spawned_in_job() -> bool {
    SPAWNED_IN_JOB.with(|s| s.get())
}

pub struct PsuedoCon {
    con: HPCON,
    funcs: &'static ConPtyFuncs,
    host: ConPtyHost,
}

unsafe impl Send for PsuedoCon {}
unsafe impl Sync for PsuedoCon {}

impl Drop for PsuedoCon {
    fn drop(&mut self) {
        unsafe { (self.funcs.ClosePseudoConsole)(self.con) };
    }
}

impl PsuedoCon {
    /// Create a pseudo console on the sideloaded `conpty.dll` at `sideload` (absolute path), or on the system ConPTY
    /// when `sideload` is `None` or the sideloaded DLL cannot be loaded or fails.
    pub fn new(
        size: COORD,
        input: FileDescriptor,
        output: FileDescriptor,
        sideload: Option<&Path>,
    ) -> Result<Self, Error> {
        if let Some(path) = sideload {
            match sideloaded_conpty(path) {
                // conpty.dll (1.22+) maps only INHERIT_CURSOR (and its own glyph-width bits) to OpenConsole arguments;
                // RESIZE_QUIRK and WIN32_INPUT_MODE are legacy inbox flags (OpenConsole always negotiates win32-input
                // mode with the terminal itself), so they are not passed.
                Ok(funcs) => match Self::create(funcs, size, &input, &output, PSUEDOCONSOLE_INHERIT_CURSOR) {
                    Ok(con) => return Ok(Self { con, funcs, host: ConPtyHost::Sideloaded }),
                    Err(e) => log::warn!("sideloaded conpty failed, using the system ConPTY: {:#}", e),
                },
                Err(e) => log::warn!("sideloaded conpty unavailable, using the system ConPTY: {:#}", e),
            }
        }
        let funcs = kernel_conpty()?;
        let con = Self::create(
            funcs,
            size,
            &input,
            &output,
            PSUEDOCONSOLE_INHERIT_CURSOR | PSEUDOCONSOLE_RESIZE_QUIRK | PSEUDOCONSOLE_WIN32_INPUT_MODE,
        )?;
        Ok(Self { con, funcs, host: ConPtyHost::System })
    }

    fn create(
        funcs: &ConPtyFuncs,
        size: COORD,
        input: &FileDescriptor,
        output: &FileDescriptor,
        flags: DWORD,
    ) -> Result<HPCON, Error> {
        let mut con: HPCON = INVALID_HANDLE_VALUE;
        let result = unsafe {
            (funcs.CreatePseudoConsole)(
                size,
                input.as_raw_handle() as _,
                output.as_raw_handle() as _,
                flags,
                &mut con,
            )
        };
        ensure!(
            result == S_OK,
            "failed to create psuedo console: HRESULT {}",
            result
        );
        Ok(con)
    }

    /// The ConPTY implementation this pseudo console runs on.
    pub fn host(&self) -> ConPtyHost {
        self.host
    }

    pub fn resize(&self, size: COORD) -> Result<(), Error> {
        let result = unsafe { (self.funcs.ResizePseudoConsole)(self.con, size) };
        ensure!(
            result == S_OK,
            "failed to resize console to {}x{}: HRESULT: {}",
            size.X,
            size.Y,
            result
        );
        Ok(())
    }

    pub fn spawn_command(&self, cmd: CommandBuilder) -> anyhow::Result<WinChild> {
        let mut si: STARTUPINFOEXW = unsafe { mem::zeroed() };
        si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        // Explicitly set the stdio handles as invalid handles otherwise
        // we can end up with a weird state where the spawned process can
        // inherit the explicitly redirected output handles from its parent.
        // For example, when daemonizing wezterm-mux-server, the stdio handles
        // are redirected to a log file and the spawned process would end up
        // writing its output there instead of to the pty we just created.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

        // Paneldeck patch: optionally start the process inside a Job Object (`set_spawn_job`), falling back to a
        // start without it when CreateProcessW refuses the job.
        let job = SPAWN_JOB.with(|j| j.take()).map(|h| h as HANDLE);
        SPAWNED_IN_JOB.with(|s| s.set(false));
        let jobs: Vec<HANDLE> = job.into_iter().collect();

        let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        let (mut exe, cmdline) = cmd.cmdline()?;
        let cmd_os = OsString::from_wide(&cmdline);

        let cwd = cmd.current_directory();

        let mut res = 0;
        for with_job in [true, false] {
            if with_job && jobs.is_empty() {
                continue;
            }
            let mut attrs = ProcThreadAttributeList::with_capacity(if with_job { 2 } else { 1 })?;
            attrs.set_pty(self.con)?;
            if with_job && attrs.set_job_list(&jobs).is_err() {
                continue;
            }
            si.lpAttributeList = attrs.as_mut_ptr();
            let mut cmdline_try = cmdline.clone();
            res = unsafe {
                CreateProcessW(
                    exe.as_mut_slice().as_mut_ptr(),
                    cmdline_try.as_mut_slice().as_mut_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                    cmd.environment_block().as_mut_slice().as_mut_ptr() as *mut _,
                    cwd.as_ref()
                        .map(|c| c.as_slice().as_ptr())
                        .unwrap_or(ptr::null()),
                    &mut si.StartupInfo,
                    &mut pi,
                )
            };
            si.lpAttributeList = ptr::null_mut();
            if res != 0 {
                SPAWNED_IN_JOB.with(|s| s.set(with_job));
                break;
            }
            if with_job {
                log::warn!("CreateProcessW with a job list failed, starting without it: {}", IoError::last_os_error());
            }
        }
        if res == 0 {
            let err = IoError::last_os_error();
            let msg = format!(
                "CreateProcessW `{:?}` in cwd `{:?}` failed: {}",
                cmd_os,
                cwd.as_ref().map(|c| OsString::from_wide(c)),
                err
            );
            log::error!("{}", msg);
            bail!("{}", msg);
        }

        // Make sure we close out the thread handle so we don't leak it;
        // we do this simply by making it owned
        let _main_thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread as _) };
        let proc = unsafe { OwnedHandle::from_raw_handle(pi.hProcess as _) };

        Ok(WinChild {
            proc: Mutex::new(proc),
        })
    }
}
