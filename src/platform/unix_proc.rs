//! Caller attribution and session "jobs" on Linux and macOS (#149/#150, release blocker).
//!
//! Windows finds the process at the other end of a local TCP connection in the TCP table and ties it to a session
//! through the session's Job Object. Here:
//!
//! * **Socket owner:** Linux reads `/proc/net/tcp` (and `tcp6` for IPv4-mapped sockets) for the inode of the client's
//!   socket (local address = the client, remote address = the hub) and looks for the process holding that inode as a
//!   file descriptor (`/proc/<pid>/fd/*` → `socket:[<inode>]`). macOS asks libproc for the TCP sockets of each process
//!   (`proc_pidinfo(PROC_PIDLISTFDS)`, `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)`). The owner is looked up twice; between
//!   the two lookups its start time must not change (the PID is not reused).
//! * **Fail closed:** no row, no owner (the client has closed its socket or exited), several owners in different
//!   processes that do not belong together, or a process that cannot be read → `None`: the hub refuses the request.
//! * **Session "job":** there are no Job Objects. A caller belongs to a session only through the **process tree**
//!   (the session's shell or one of its descendants). On Linux the hub is a child subreaper ([`become_subreaper`]):
//!   descendants that detach (`setsid`, double fork, `env -i …`) are re-parented to the hub, so their tree still ends
//!   there – such a caller is "orphan" (belongs to Paneldeck, but to no session it can prove) and the MCP rules restrict
//!   it; it is never an external client. Each session also gets a random tag in its environment ([`JOB_ENV`]): it is
//!   readable (and forgeable) by every process of the same user, so it only ever *restricts* (a caller with the tag of
//!   a session but outside its tree is "orphan") and lists processes for "close session"; it never grants anything.
//!   Without a subreaper (macOS) a tree that breaks at launchd below a process started after the hub counts as
//!   "orphan", like on Windows. As on Windows this is no boundary against hostile code of the same user; the chain
//!   rules protect against steered agents, not local malware.
//! * Start times are the kernel's (`/proc/<pid>/stat` field 22 in clock ticks since boot; macOS microseconds since
//!   the epoch). They are only compared with each other.

use super::process::PeerChain;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// Environment variable with the session tag (internal, see `terminal::INTERNAL_ENV`).
pub const JOB_ENV: &str = "PANELDECK_SESSION_JOB";

/// What the attribution needs to know about one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub ppid: u32,
    /// Start time (only comparable with other values of this function).
    pub start: u64,
    pub pgrp: i64,
    /// Foreground process group of the controlling terminal (`-1`/`0` = none).
    pub tpgid: i64,
    /// Controlling terminal (`0` = none).
    pub tty: u64,
    pub zombie: bool,
}

// ---------------------------------------------------------------------------------------------------------------------
// Parsing (pure, tested on every platform)

/// One line of `/proc/<pid>/stat`. The command name (in parentheses) may contain spaces and `)`: the fields start
/// after the LAST `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_stat(line: &str) -> Option<Stat> {
    let rest = &line[line.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[0] state, f[1] ppid, f[2] pgrp, f[3] session, f[4] tty_nr, f[5] tpgid, … f[19] starttime
    if f.len() < 20 {
        return None;
    }
    Some(Stat {
        ppid: f[1].parse().ok()?,
        start: f[19].parse().ok()?,
        pgrp: f[2].parse().ok()?,
        tpgid: f[5].parse().ok()?,
        tty: f[4].parse::<i64>().ok()? as u64,
        zombie: matches!(f[0], "Z" | "X" | "x"),
    })
}

/// `0100007F:1F90` (IPv4, the address as the kernel's native 32-bit word in hex, the port in hex) or the 32-digit
/// IPv6 form of `/proc/net/tcp6` (four native words). IPv6 counts only as an IPv4-mapped address.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_proc_addr(s: &str) -> Option<SocketAddrV4> {
    let (ip, port) = s.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |h: &str| u32::from_str_radix(h, 16).ok().map(u32::to_ne_bytes);
    let ip = match ip.len() {
        8 => Ipv4Addr::from(word(ip)?),
        32 => {
            let mut b = [0u8; 16];
            for i in 0..4 {
                b[i * 4..i * 4 + 4].copy_from_slice(&word(&ip[i * 8..i * 8 + 8])?);
            }
            std::net::Ipv6Addr::from(b).to_ipv4_mapped()?
        }
        _ => return None,
    };
    Some(SocketAddrV4::new(ip, port))
}

/// Inodes of the sockets in a `/proc/net/tcp*` table whose local address is `peer` and remote address `local`
/// (any state: a client that has sent its request and shut down writing is in `FIN_WAIT2`). Inode 0 = the socket has
/// no file any more (closed by its process) – not a match.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn inodes_in_table(table: &str, peer: SocketAddrV4, local: SocketAddrV4) -> Vec<u64> {
    table
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 || parse_proc_addr(f[1])? != peer || parse_proc_addr(f[2])? != local {
                return None;
            }
            f[9].parse::<u64>().ok().filter(|i| *i != 0)
        })
        .collect()
}

/// Value of `name` in a NUL-separated environment block.
pub fn env_value(block: &[u8], name: &str) -> Option<String> {
    block.split(|b| *b == 0).find_map(|kv| {
        let rest = kv.strip_prefix(name.as_bytes())?.strip_prefix(b"=")?;
        Some(String::from_utf8_lossy(rest).into_owned())
    })
}

/// Environment part of a macOS `KERN_PROCARGS2` buffer: `argc` (i32, native), the executable path, padding NULs,
/// `argc` arguments, then the environment until an empty string.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn procargs_env(buf: &[u8]) -> Option<Vec<u8>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let mut i = 4;
    let skip_str = |i: &mut usize| {
        while *i < buf.len() && buf[*i] != 0 {
            *i += 1;
        }
    };
    skip_str(&mut i); // executable path
    while i < buf.len() && buf[i] == 0 {
        i += 1;
    }
    for _ in 0..argc.max(0) {
        skip_str(&mut i);
        i += 1;
    }
    let start = i.min(buf.len());
    let mut end = start;
    while end < buf.len() {
        let s = end;
        while end < buf.len() && buf[end] != 0 {
            end += 1;
        }
        if end == s {
            break;
        }
        end += 1;
    }
    Some(buf[start..end.min(buf.len())].to_vec())
}

// ---------------------------------------------------------------------------------------------------------------------
// System access: Linux

#[cfg(target_os = "linux")]
mod sys {
    use super::*;

    pub fn stat(pid: u32) -> Option<Stat> {
        parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    pub fn all_pids() -> Vec<u32> {
        let Ok(rd) = std::fs::read_dir("/proc") else { return Vec::new() };
        rd.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).collect()
    }

    pub fn name(pid: u32) -> Option<String> {
        Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim_end().to_string())
    }

    pub fn environ(pid: u32) -> Option<Vec<u8>> {
        std::fs::read(format!("/proc/{pid}/environ")).ok()
    }

    /// Processes that hold the client socket of the connection `peer` → `local` as a file descriptor.
    pub fn socket_owners(peer: SocketAddrV4, local: SocketAddrV4) -> Vec<u32> {
        let mut inodes = Vec::new();
        for t in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(s) = std::fs::read_to_string(t) {
                inodes.extend(inodes_in_table(&s, peer, local));
            }
        }
        if inodes.is_empty() {
            return Vec::new();
        }
        let want: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
        let mut out = Vec::new();
        for pid in all_pids() {
            let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
            for e in rd.flatten() {
                if std::fs::read_link(e.path()).is_ok_and(|l| want.iter().any(|w| l.as_os_str() == w.as_str())) {
                    out.push(pid);
                    break;
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// System access: macOS (libproc, sysctl)

#[cfg(target_os = "macos")]
mod sys {
    use super::*;
    use std::ffi::c_void;

    /// `PROC_PIDFDSOCKETINFO` (sys/proc_info.h); not in the libc crate.
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    /// `SOCKINFO_TCP` (`soi_kind`).
    const SOCKINFO_TCP: i32 = 2;
    /// `INI_IPV4` (`insi_vflag`).
    const INI_IPV4: u8 = 0x1;
    // Offsets in `struct socket_fdinfo` (proc_fileinfo = 24 bytes, then socket_info; vinfo_stat = 136 bytes):
    const OFF_FAMILY: usize = 24 + 160; // soi_family
    const OFF_KIND: usize = 24 + 232; // soi_kind
    const OFF_PROTO: usize = 24 + 240; // soi_proto (in_sockinfo / tcp_sockinfo)
    const OFF_FPORT: usize = OFF_PROTO; // insi_fport (network order in the low 16 bits)
    const OFF_LPORT: usize = OFF_PROTO + 4; // insi_lport
    const OFF_VFLAG: usize = OFF_PROTO + 24; // insi_vflag
    const OFF_FADDR4: usize = OFF_PROTO + 32 + 12; // insi_faddr.ina_46.i46a_addr4
    const OFF_LADDR4: usize = OFF_PROTO + 48 + 12; // insi_laddr.ina_46.i46a_addr4

    fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
        // SAFETY: zeroed plain-old-data struct, passed with its exact size; the result is checked.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
            let n = libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size);
            (n == size).then_some(info)
        }
    }

    pub fn stat(pid: u32) -> Option<Stat> {
        let i = bsdinfo(pid)?;
        Some(Stat {
            ppid: i.pbi_ppid,
            start: i.pbi_start_tvsec.saturating_mul(1_000_000).saturating_add(i.pbi_start_tvusec),
            pgrp: i.pbi_pgid as i64,
            tpgid: i.e_tpgid as i64,
            tty: if i.e_tdev == u32::MAX { 0 } else { i.e_tdev as u64 },
            zombie: i.pbi_status == libc::SZOMB,
        })
    }

    pub fn all_pids() -> Vec<u32> {
        // SAFETY: first call asks for the count (null buffer), the second fills a buffer of that size (+ reserve).
        unsafe {
            let n = libc::proc_listallpids(std::ptr::null_mut(), 0);
            if n <= 0 {
                return Vec::new();
            }
            let mut buf = vec![0i32; n as usize + 64];
            let got = libc::proc_listallpids(buf.as_mut_ptr() as *mut c_void, (buf.len() * 4) as i32);
            if got <= 0 {
                return Vec::new();
            }
            buf.truncate(got as usize);
            buf.into_iter().filter(|p| *p > 0).map(|p| p as u32).collect()
        }
    }

    pub fn name(pid: u32) -> Option<String> {
        let i = bsdinfo(pid)?;
        let raw = if i.pbi_name[0] != 0 { &i.pbi_name[..] } else { &i.pbi_comm[..] };
        let bytes: Vec<u8> = raw.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn environ(pid: u32) -> Option<Vec<u8>> {
        // SAFETY: sysctl with a local MIB and buffers of the reported size.
        unsafe {
            let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
            let mut argmax: i32 = 0;
            let mut size = std::mem::size_of::<i32>();
            if libc::sysctl(mib.as_mut_ptr(), 2, &mut argmax as *mut _ as *mut c_void, &mut size, std::ptr::null_mut(), 0) != 0 || argmax <= 0 {
                return None;
            }
            let mut buf = vec![0u8; argmax as usize];
            let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
            let mut size = buf.len();
            if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut c_void, &mut size, std::ptr::null_mut(), 0) != 0 {
                return None;
            }
            buf.truncate(size);
            procargs_env(&buf)
        }
    }

    fn fds(pid: u32) -> Vec<libc::proc_fdinfo> {
        // SAFETY: size query first, then a buffer of proc_fdinfo entries of (at least) that size.
        unsafe {
            let n = libc::proc_pidinfo(pid as i32, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
            if n <= 0 {
                return Vec::new();
            }
            let each = std::mem::size_of::<libc::proc_fdinfo>();
            let mut buf: Vec<libc::proc_fdinfo> = vec![std::mem::zeroed(); n as usize / each + 16];
            let got = libc::proc_pidinfo(pid as i32, libc::PROC_PIDLISTFDS, 0, buf.as_mut_ptr() as *mut c_void, (buf.len() * each) as i32);
            if got <= 0 {
                return Vec::new();
            }
            buf.truncate(got as usize / each);
            buf
        }
    }

    fn rd_i32(b: &[u8], o: usize) -> i32 {
        i32::from_ne_bytes(b[o..o + 4].try_into().unwrap_or([0; 4]))
    }

    /// Does this TCP socket of `pid` go from `peer` (local side) to `local` (remote side)?
    fn socket_matches(pid: u32, fd: i32, peer: SocketAddrV4, local: SocketAddrV4) -> bool {
        // Large, 8-byte aligned buffer (socket_fdinfo is about 800 bytes); the kernel writes its size and returns it.
        let mut buf = vec![0u64; 256];
        let cap = (buf.len() * 8) as i32;
        // SAFETY: buffer of `cap` bytes; the result says how many were written.
        let n = unsafe { libc::proc_pidfdinfo(pid as i32, fd, PROC_PIDFDSOCKETINFO, buf.as_mut_ptr() as *mut c_void, cap) };
        if n <= 0 || (n as usize) < OFF_LADDR4 + 4 {
            return false;
        }
        // SAFETY: reinterpret the u64 buffer as bytes (same allocation, `n` ≤ its size).
        let b = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, n as usize) };
        if rd_i32(b, OFF_FAMILY) != libc::AF_INET || rd_i32(b, OFF_KIND) != SOCKINFO_TCP || b[OFF_VFLAG] & INI_IPV4 == 0 {
            return false;
        }
        let port = |o: usize| u16::from_be(rd_i32(b, o) as u16);
        let addr = |o: usize| Ipv4Addr::new(b[o], b[o + 1], b[o + 2], b[o + 3]);
        port(OFF_LPORT) == peer.port() && port(OFF_FPORT) == local.port() && addr(OFF_LADDR4) == *peer.ip() && addr(OFF_FADDR4) == *local.ip()
    }

    pub fn socket_owners(peer: SocketAddrV4, local: SocketAddrV4) -> Vec<u32> {
        all_pids()
            .into_iter()
            .filter(|&pid| fds(pid).iter().any(|f| f.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32 && socket_matches(pid, f.proc_fd, peer, local)))
            .collect()
    }
}

// Other Unix targets: nothing can be attributed – every caller is refused.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod sys {
    use super::*;
    pub fn stat(_pid: u32) -> Option<Stat> {
        None
    }
    pub fn all_pids() -> Vec<u32> {
        Vec::new()
    }
    pub fn name(_pid: u32) -> Option<String> {
        None
    }
    pub fn environ(_pid: u32) -> Option<Vec<u8>> {
        None
    }
    pub fn socket_owners(_peer: SocketAddrV4, _local: SocketAddrV4) -> Vec<u32> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Session jobs

struct Job {
    key: u64,
    /// Random tag in the session's environment ([`JOB_ENV`]).
    tag: String,
    /// The session's shell (PID, start time) once started.
    shell: Option<(u32, u64)>,
}

fn jobs() -> std::sync::MutexGuard<'static, Vec<Job>> {
    static JOBS: std::sync::Mutex<Vec<Job>> = std::sync::Mutex::new(Vec::new());
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

fn random_tag() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

pub fn job_new() -> Option<u64> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let tag = random_tag()?;
    let key = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    jobs().push(Job { key, tag, shell: None });
    Some(key)
}

pub fn job_env(key: u64) -> Option<(&'static str, String)> {
    jobs().iter().find(|j| j.key == key).map(|j| (JOB_ENV, j.tag.clone()))
}

/// Remember the shell of the session (started with the tag in its environment).
pub fn job_adopt(key: u64, pid: u32) -> bool {
    let Some(start) = sys::stat(pid).map(|s| s.start) else { return false };
    let mut g = jobs();
    let Some(j) = g.iter_mut().find(|j| j.key == key) else { return false };
    j.shell = Some((pid, start));
    true
}

pub fn job_drop(key: u64) {
    jobs().retain(|j| j.key != key);
    pids_cache().remove(&key);
}

/// Snapshot of all processes: PID → stat.
fn snapshot() -> HashMap<u32, Stat> {
    sys::all_pids().into_iter().filter_map(|p| Some((p, sys::stat(p)?))).collect()
}

/// Is `pid` (started at `born`) the shell or a descendant of the shell `shell` (PID, start)? `pid` must be in the
/// snapshot with exactly this start time, and every parent must have started no later than its child (otherwise its
/// PID was reused).
fn below(snap: &HashMap<u32, Stat>, pid: u32, born: u64, shell: (u32, u64)) -> bool {
    if snap.get(&pid).is_none_or(|s| s.start != born) {
        return false;
    }
    let (mut cur, mut cur_start) = (pid, born);
    for _ in 0..64 {
        if cur == shell.0 {
            return cur_start == shell.1;
        }
        let Some(s) = snap.get(&cur) else { return false };
        let p = s.ppid;
        if p == 0 || p == cur {
            return false;
        }
        match snap.get(&p) {
            Some(ps) if ps.start <= cur_start => {
                cur = p;
                cur_start = ps.start;
            }
            _ => return false,
        }
    }
    false
}

/// Processes of a job: the shell, its descendants and every process with the job's tag (the tag only serves this
/// listing for "close session" and never grants anything, see [`resolve_peer`]).
fn compute_job_pids(tag: &str, shell: Option<(u32, u64)>) -> Option<Vec<u32>> {
    let snap = snapshot();
    if snap.is_empty() {
        return None;
    }
    let me = std::process::id();
    let mut out: Vec<u32> = snap
        .iter()
        .filter(|(p, s)| **p != me && !s.zombie)
        .filter(|(p, s)| shell.is_some_and(|sh| below(&snap, **p, s.start, sh)) || sys::environ(**p).and_then(|e| env_value(&e, JOB_ENV)).is_some_and(|t| t == tag))
        .map(|(p, _)| *p)
        .collect();
    out.sort_unstable();
    Some(out)
}

fn job_info(key: u64) -> Option<(String, Option<(u32, u64)>)> {
    jobs().iter().find(|j| j.key == key).map(|j| (j.tag.clone(), j.shell))
}

type PidsCache = HashMap<u64, (std::time::Instant, Vec<u32>)>;

fn pids_cache() -> std::sync::MutexGuard<'static, PidsCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<PidsCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Running processes of the job, for the UI (asked about once a second while a closed session is watched): served from
/// a cache that a background thread refreshes every 1.5 s, so the UI thread does not read every process's environment.
/// Only the very first request for a job is computed at once.
pub fn job_pids(key: u64) -> Option<Vec<u32>> {
    static REFRESHER: std::sync::Once = std::sync::Once::new();
    REFRESHER.call_once(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let wanted: Vec<u64> = pids_cache().keys().copied().collect();
            for k in wanted {
                let Some((tag, shell)) = job_info(k) else { continue };
                if let Some(v) = compute_job_pids(&tag, shell) {
                    if job_info(k).is_some() {
                        pids_cache().insert(k, (std::time::Instant::now(), v));
                    }
                }
            }
        });
    });
    if let Some((t, v)) = pids_cache().get(&key) {
        if t.elapsed() < std::time::Duration::from_secs(5) {
            return Some(v.clone());
        }
    }
    let (tag, shell) = job_info(key)?;
    let v = compute_job_pids(&tag, shell)?;
    pids_cache().insert(key, (std::time::Instant::now(), v.clone()));
    Some(v)
}

/// Fresh list (for killing: never act on a cached one).
fn job_pids_now(key: u64) -> Option<Vec<u32>> {
    let (tag, shell) = job_info(key)?;
    compute_job_pids(&tag, shell)
}

fn kill(pid: u32) -> bool {
    // SAFETY: plain system call.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
}

pub fn job_terminate_all(key: u64) -> bool {
    let Some(pids) = job_pids_now(key) else { return false };
    for p in pids {
        kill(p);
    }
    true
}

pub fn job_terminate_listed(key: u64, procs: &[(u32, u64)]) -> usize {
    let Some(pids) = job_pids_now(key) else { return 0 };
    procs.iter().filter(|(p, born)| pids.contains(p) && process_born(*p) == Some(*born) && kill(*p)).count()
}

// ---------------------------------------------------------------------------------------------------------------------
// Subreaper (Linux)

static SUBREAPER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Linux: make the hub a child subreaper (`PR_SET_CHILD_SUBREAPER`). Descendants of the sessions that detach (`setsid`,
/// double fork, `nohup … &` of a shell that ends) are then re-parented to the hub instead of init, so their tree still
/// ends at the hub: [`resolve_peer`] marks such a caller "orphan" (it belongs to Paneldeck but to no session it can
/// prove) and the MCP rules restrict it. A background thread reaps those orphans when they end (only zombies that are
/// not session shells and have stayed zombies for 5 s – the hub's own child processes are waited for well before).
/// Call once, in the hub process only (not in the MCP bridge or other short-lived modes). macOS has no subreaper.
pub fn become_subreaper() -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: plain prctl call on this process.
        if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1 as libc::c_ulong, 0, 0, 0) } != 0 {
            return false;
        }
        SUBREAPER.store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::spawn(reaper);
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn reaper() {
    let me = std::process::id();
    let mut seen: HashMap<u32, std::time::Instant> = HashMap::new();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let shells: Vec<u32> = jobs().iter().filter_map(|j| j.shell.map(|s| s.0)).collect();
        let zombies: Vec<u32> = sys::all_pids().into_iter().filter(|p| sys::stat(*p).is_some_and(|s| s.ppid == me && s.zombie) && !shells.contains(p)).collect();
        seen.retain(|p, _| zombies.contains(p));
        for p in zombies {
            let first = *seen.entry(p).or_insert_with(std::time::Instant::now);
            if first.elapsed() >= std::time::Duration::from_secs(5) {
                let mut status = 0;
                // SAFETY: reaps exactly this child (no -1), never blocks.
                unsafe { libc::waitpid(p as libc::pid_t, &mut status, libc::WNOHANG) };
                seen.remove(&p);
            }
        }
    }
}

/// Start time of this process (the hub).
fn hub_started() -> u64 {
    static T: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *T.get_or_init(|| sys::stat(std::process::id()).map(|s| s.start).unwrap_or(0))
}

// ---------------------------------------------------------------------------------------------------------------------
// Public functions used by `process`

pub fn process_born(pid: u32) -> Option<u64> {
    sys::stat(pid).filter(|s| !s.zombie).map(|s| s.start)
}

/// Owner of the client socket `peer` → `local`. Several processes can hold the same socket (an inherited descriptor):
/// the one started last is taken, but only if every other holder is one of its ancestors (a parent that forked it). A
/// socket that unrelated processes share (e.g. passed on with `SCM_RIGHTS`) is not attributed: `None`. `None` as well if
/// there is no owner or only this process (outside the tests).
pub fn tcp_peer_pid(peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    let (SocketAddr::V4(peer), SocketAddr::V4(local)) = (peer, local) else { return None };
    let me = std::process::id();
    let mut owners: Vec<(u32, u64)> = sys::socket_owners(peer, local).into_iter().filter(|p| *p != me || cfg!(test)).filter_map(|p| Some((p, process_born(p)?))).collect();
    owners.sort_by_key(|(_, t)| *t);
    let (pid, born) = *owners.last()?;
    if owners.len() > 1 {
        let chain = walk(&snapshot(), pid, born);
        if !owners[..owners.len() - 1].iter().all(|(p, _)| chain.contains(p)) {
            return None;
        }
    }
    Some(pid)
}

/// Linux: a pidfd holds on to the process (no PID reuse in between); at the end it tells whether it still runs.
#[cfg(target_os = "linux")]
struct PidFd(i32);

#[cfg(target_os = "linux")]
impl PidFd {
    fn open(pid: u32) -> Option<PidFd> {
        // SAFETY: plain system call; a negative result is an error (e.g. ENOSYS before Linux 5.3).
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        (fd >= 0).then_some(PidFd(fd as i32))
    }
    fn alive(&self) -> bool {
        // SAFETY: signal 0 only checks; valid fd owned by this struct.
        unsafe { libc::syscall(libc::SYS_pidfd_send_signal, self.0, 0, std::ptr::null::<libc::siginfo_t>(), 0) == 0 }
    }
}

#[cfg(target_os = "linux")]
impl Drop for PidFd {
    fn drop(&mut self) {
        // SAFETY: own fd, closed once.
        unsafe { libc::close(self.0) };
    }
}

/// Caller of an accepted connection (see the module text). `None` = refuse.
///
/// * The caller belongs to a session only through the **process tree** (`job` = that session): it is the session's
///   shell or one of its descendants.
/// * `orphan` (restricted by the MCP rules: writes nowhere in a chain, starts no agents) if it belongs to Paneldeck but
///   to no session it can prove: its tree ends at the hub (detached descendant re-parented to the subreaper hub), it
///   carries the tag of a session of this hub (detached, tree lost – the tag can only restrict, never grant), or –
///   without a subreaper (macOS, or `prctl` failed) – its tree breaks at init/launchd with a process that started after
///   the hub (like the orphan rule on Windows; this also hits programs started later by the desktop itself, e.g. a
///   terminal app opened after Paneldeck).
/// * Otherwise it is an external client. Known limit (like WMI/schtasks on Windows): a process that a session asks
///   another service to start (`systemd-run --user`, `at`, D-Bus activation, launchd jobs) and that clears the tag is
///   not a descendant of anything here and counts as external.
pub fn resolve_peer(peer: SocketAddr, local: SocketAddr) -> Option<PeerChain> {
    let pid = tcp_peer_pid(peer, local)?;
    let born = process_born(pid)?;
    #[cfg(target_os = "linux")]
    let pidfd = PidFd::open(pid);
    // Second look: the socket still belongs to the same process (same PID, same start time).
    if tcp_peer_pid(peer, local) != Some(pid) || process_born(pid) != Some(born) {
        return None;
    }
    let snap = snapshot();
    // The caller must be in the snapshot as this very process – otherwise it has just ended: refuse (never "external").
    snap.get(&pid).filter(|s| s.start == born && !s.zombie)?;
    let chain = walk(&snap, pid, born);
    let registered: Vec<(u64, String, Option<(u32, u64)>)> = jobs().iter().map(|j| (j.key, j.tag.clone(), j.shell)).collect();
    let job = registered.iter().find(|(_, _, sh)| sh.is_some_and(|sh| below(&snap, pid, born, sh))).map(|(k, ..)| *k);
    let mut orphan = false;
    if job.is_none() {
        let me = std::process::id();
        if chain.iter().skip(1).any(|p| *p == me) {
            orphan = true;
        }
        let tag = sys::environ(pid).and_then(|e| env_value(&e, JOB_ENV));
        if process_born(pid) != Some(born) {
            return None;
        }
        if tag.is_some_and(|t| registered.iter().any(|(_, rt, _)| *rt == t)) {
            orphan = true;
        }
        if !SUBREAPER.load(std::sync::atomic::Ordering::SeqCst) && detached_after(&snap, &chain, hub_started()) {
            orphan = true;
        }
    }
    #[cfg(target_os = "linux")]
    if pidfd.as_ref().is_some_and(|f| !f.alive()) {
        return None;
    }
    if process_born(pid) != Some(born) {
        return None;
    }
    Some(PeerChain { chain, orphan, job })
}

/// Does the tree `chain` break at init/launchd (PID 1) below a process that started at or after `hub_born`? The last
/// process before PID 1 is the one whose real parent is gone (or that the desktop started directly).
fn detached_after(snap: &HashMap<u32, Stat>, chain: &[u32], hub_born: u64) -> bool {
    let top = match chain {
        [.., top, 1] => *top,
        [only] if snap.get(only).is_some_and(|s| s.ppid == 1) => *only,
        _ => return false,
    };
    snap.get(&top).is_some_and(|s| s.start >= hub_born)
}

/// Ancestors of `pid` (start `born`), nearest first, each started no later than its child.
fn walk(snap: &HashMap<u32, Stat>, pid: u32, born: u64) -> Vec<u32> {
    let mut chain = vec![pid];
    if snap.get(&pid).is_some_and(|s| s.start != born) {
        return chain;
    }
    let (mut cur, mut cur_born) = (pid, born);
    for _ in 0..64 {
        let Some(p) = snap.get(&cur).map(|s| s.ppid) else { break };
        if p == 0 || chain.contains(&p) {
            break;
        }
        match snap.get(&p) {
            Some(ps) if ps.start <= cur_born => {
                chain.push(p);
                cur = p;
                cur_born = ps.start;
            }
            _ => break,
        }
    }
    chain
}

/// Is the shell in the foreground of its terminal (nothing running in it)? The foreground process group of the
/// controlling terminal is the shell's own group, and no other process is in that group (a shell without job control
/// runs its commands in its own group). `None` if unknown.
///
/// Limit: a program can put itself into the background (`tcsetpgrp` back to the shell) and keep running, and a short
/// command can end between two looks; this is a snapshot, not a guarantee. Paneldeck does not use it on Linux/macOS
/// yet (`shells::foreground_visible` is Windows-only).
pub fn shell_is_idle(shell: u32) -> Option<bool> {
    let s = sys::stat(shell).filter(|s| !s.zombie)?;
    if s.tty == 0 || s.tpgid <= 0 {
        return None;
    }
    if s.tpgid != s.pgrp {
        return Some(false);
    }
    Some(!snapshot().iter().any(|(p, o)| *p != shell && !o.zombie && o.pgrp == s.pgrp))
}

/// Processes on the same controlling terminal as `pid` (they get SIGHUP when the session's terminal closes).
pub fn console_pids(pid: u32) -> Option<Vec<u32>> {
    let tty = sys::stat(pid).filter(|s| s.tty != 0)?.tty;
    let mut v: Vec<u32> = snapshot().into_iter().filter(|(_, s)| s.tty == tty && !s.zombie).map(|(p, _)| p).collect();
    v.sort_unstable();
    Some(v)
}

pub fn job_parents(pids: &[u32]) -> HashMap<u32, u32> {
    let snap = snapshot();
    pids.iter()
        .filter_map(|c| {
            let cs = snap.get(c)?;
            let ps = snap.get(&cs.ppid)?;
            (cs.ppid != *c && pids.contains(&cs.ppid) && ps.start <= cs.start).then_some((*c, cs.ppid))
        })
        .collect()
}

pub fn process_names() -> HashMap<u32, String> {
    sys::all_pids().into_iter().filter_map(|p| Some((p, sys::name(p)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines() {
        let l = "4242 (my (odd) prog) S 100 4242 4242 34816 4242 4194560 1 2 3 4 5 6 7 8 20 0 1 0 987654 1000 10";
        let s = parse_stat(l).unwrap();
        assert_eq!((s.ppid, s.pgrp, s.tty, s.tpgid, s.start, s.zombie), (100, 4242, 34816, 4242, 987654, false));
        assert!(parse_stat("1 (z) Z 0 0 0 0 -1 0 0 0 0 0 0 0 0 0 0 0 1 0 5 0 0").unwrap().zombie);
        assert_eq!(parse_stat("1 (short) S 0 0"), None);
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn proc_net_addresses() {
        let native = |a: [u8; 4]| format!("{:08X}", u32::from_ne_bytes(a));
        let s = format!("{}:1F90", native([127, 0, 0, 1]));
        assert_eq!(parse_proc_addr(&s), Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8080)));
        // IPv4-mapped in tcp6: ::ffff:127.0.0.1
        let words = [[0u8, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0xff, 0xff], [127, 0, 0, 1]];
        let v6: String = words.iter().map(|w| native(*w)).collect();
        assert_eq!(parse_proc_addr(&format!("{v6}:0050")), Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 80)));
        // real IPv6 (::1) does not count
        let lo6: String = [[0u8, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1]].iter().map(|w| native(*w)).collect();
        assert_eq!(parse_proc_addr(&format!("{lo6}:0050")), None);
        assert_eq!(parse_proc_addr("XYZ:1"), None);
    }

    #[test]
    fn table_rows_match_exactly() {
        let native = |a: [u8; 4]| format!("{:08X}", u32::from_ne_bytes(a));
        let lo = native([127, 0, 0, 1]);
        let table = format!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: {lo}:C350 {lo}:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 555 1 0000\n\
             1: {lo}:1F90 {lo}:C350 01 00000000:00000000 00:00000000 00000000  1000        0 666 1 0000\n\
             2: {lo}:C351 {lo}:1F90 05 00000000:00000000 00:00000000 00000000  1000        0 0 1 0000\n"
        );
        let (client, hub) = (SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50000), SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8080));
        assert_eq!(inodes_in_table(&table, client, hub), [555]);
        // closed client socket (inode 0) is no owner
        assert!(inodes_in_table(&table, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50001), hub).is_empty());
        assert!(inodes_in_table(&table, hub, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1)).is_empty());
    }

    #[test]
    fn environment_blocks() {
        let b = b"PATH=/bin\0PANELDECK_SESSION_JOB=abc\0PANELDECK_SESSION_JOBX=no\0";
        assert_eq!(env_value(b, JOB_ENV).as_deref(), Some("abc"));
        assert_eq!(env_value(b, "HOME"), None);
        let mut pa = 2i32.to_ne_bytes().to_vec();
        pa.extend_from_slice(b"/bin/x\0\0\0\0/bin/x\0-a\0A=1\0B=2\0\0junk");
        assert_eq!(procargs_env(&pa).unwrap(), b"A=1\0B=2\0");
        assert_eq!(env_value(&procargs_env(&pa).unwrap(), "B").as_deref(), Some("2"));
    }
}

/// Tests against the real system (Linux and macOS CI jobs).
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod sys_tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn own_socket_is_found_and_closed_one_is_refused() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let c = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, _) = l.accept().unwrap();
        let (peer, local) = (s.peer_addr().unwrap(), s.local_addr().unwrap());
        assert_eq!(tcp_peer_pid(peer, local), Some(std::process::id()));
        let pc = resolve_peer(peer, local).expect("living caller");
        assert_eq!(pc.chain.first(), Some(&std::process::id()));
        assert!(!pc.orphan);
        assert_eq!(pc.job, None, "the test process is in no session");
        // unknown ports → nothing
        let unknown: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(resolve_peer(unknown, local), None);
        assert_eq!(resolve_peer(peer, unknown), None);
        // the client closes its socket: nobody owns it any more → refused (fail closed)
        drop(c);
        let mut b = [0u8; 1];
        let _ = (&s).read(&mut b);
        assert_eq!(resolve_peer(peer, local), None);
    }

    #[test]
    fn child_process_and_its_job_tag() {
        let key = job_new().expect("job");
        let (name, tag) = job_env(key).unwrap();
        assert_eq!(name, JOB_ENV);
        assert_eq!(tag.len(), 32);
        // A child with the tag, detached from nothing in particular: found via its environment.
        let mut child = std::process::Command::new("sleep").arg("30").env(JOB_ENV, &tag).spawn().unwrap();
        let pid = child.id();
        // wait until exec has happened (environ shows the tag)
        for _ in 0..100 {
            if sys::environ(pid).and_then(|e| env_value(&e, JOB_ENV)).as_deref() == Some(tag.as_str()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let pids = job_pids(key).unwrap();
        assert!(pids.contains(&pid), "{pids:?}");
        assert!(!pids.contains(&std::process::id()));
        assert!(process_born(pid).is_some());
        assert_eq!(job_terminate_listed(key, &[(pid, process_born(pid).unwrap() + 1)]), 0, "wrong start time: not killed");
        assert_eq!(job_terminate_listed(key, &[(pid, process_born(pid).unwrap())]), 1);
        let _ = child.wait();
        job_drop(key);
        assert_eq!(job_pids(key), None);
        assert_eq!(job_env(key), None);
    }

    #[test]
    fn shell_tree_and_tag_attribution() {
        // "Shell" = a child we start; its child connects to a listener of this process.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let key = job_new().expect("job");
        let other = job_new().expect("second job");
        let (_, tag) = job_env(key).unwrap();
        let (_, other_tag) = job_env(other).unwrap();
        // The shell carries `tag`; its child claims the OTHER job's tag – the tree decides.
        let script = format!("exec 3<>/dev/tcp/127.0.0.1/{port}; printf x >&3; sleep 30");
        let mut shell = std::process::Command::new("bash")
            .args(["-c", &format!("{JOB_ENV}={other_tag} bash -c '{script}' & wait")])
            .env(JOB_ENV, &tag)
            .spawn()
            .unwrap();
        assert!(job_adopt(key, shell.id()));
        let (mut s, _) = l.accept().unwrap();
        let mut b = [0u8; 1];
        s.read_exact(&mut b).unwrap();
        let pc = resolve_peer(s.peer_addr().unwrap(), s.local_addr().unwrap()).expect("caller");
        assert!(pc.chain.contains(&shell.id()), "{pc:?}");
        assert_eq!(pc.job, Some(key), "tree beats a foreign tag");
        assert!(!pc.orphan);
        // Without the shell registered (e.g. it has gone), the tag never grants a session – it only restricts.
        job_drop(key);
        let pc = resolve_peer(s.peer_addr().unwrap(), s.local_addr().unwrap()).expect("caller");
        assert_eq!(pc.job, None);
        assert!(pc.orphan, "a caller with the tag of a session but outside its tree is restricted");
        let _ = s.write_all(b"x");
        job_terminate_all(other);
        let _ = shell.kill();
        let _ = shell.wait();
        job_drop(other);
    }
}

/// Unit tests of the tree rules with a made-up process table.
#[cfg(test)]
mod tree_tests {
    use super::*;

    fn st(ppid: u32, start: u64) -> Stat {
        Stat { ppid, start, pgrp: 0, tpgid: 0, tty: 0, zombie: false }
    }

    #[test]
    fn reused_pids_and_vanished_callers_are_never_matched() {
        // 1 (init) → 10 (shell, start 100) → 20 (client, start 200)
        let mut snap: HashMap<u32, Stat> = HashMap::from([(1, st(0, 1)), (10, st(1, 100)), (20, st(10, 200))]);
        assert!(below(&snap, 20, 200, (10, 100)));
        assert_eq!(walk(&snap, 20, 200), [20, 10, 1]);
        // the client is gone (not in the snapshot) or its PID belongs to another process now
        assert!(!below(&snap, 20, 199, (10, 100)));
        assert_eq!(walk(&snap, 20, 199), [20]);
        assert!(!below(&snap, 21, 200, (10, 100)));
        // the shell's PID was reused by a process started after the client
        snap.insert(10, st(1, 300));
        assert!(!below(&snap, 20, 200, (10, 100)));
        assert!(!below(&snap, 20, 200, (10, 300)));
        assert_eq!(walk(&snap, 20, 200), [20]);
    }

    #[test]
    fn broken_tree_after_the_hub_start_is_detached() {
        let snap: HashMap<u32, Stat> = HashMap::from([(1, st(0, 1)), (30, st(1, 50)), (31, st(30, 60)), (40, st(1, 500)), (41, st(40, 510))]);
        // started from a terminal that is older than the hub: not detached
        assert!(!detached_after(&snap, &walk(&snap, 31, 60), 100));
        // tree breaks at init below a process younger than the hub (double fork, setsid)
        assert!(detached_after(&snap, &walk(&snap, 41, 510), 100));
        assert!(detached_after(&snap, &walk(&snap, 40, 500), 100));
        // chain that does not end at init (unknown parent): not decided here
        let partial: HashMap<u32, Stat> = HashMap::from([(50, st(49, 600))]);
        assert!(!detached_after(&partial, &walk(&partial, 50, 600), 100));
    }
}

/// Linux (#149 review, blocker B1): a descendant of a session that clears the tag and detaches (double fork) must not
/// become an external client. With the subreaper it is re-parented to the hub (here: this test process) and is
/// "orphan".
#[cfg(all(test, target_os = "linux"))]
mod subreaper_tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn detached_descendant_without_tag_is_orphan_not_external() {
        // SAFETY: plain prctl on the test process (orphans of other tests then end up here as well – harmless).
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1 as libc::c_ulong, 0, 0, 0) }, 0);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let key = job_new().expect("job");
        let (_, tag) = job_env(key).unwrap();
        let script = format!("exec 3<>/dev/tcp/127.0.0.1/{port}; printf x >&3; sleep 30");
        // shell → subshell that starts the client in the background with an empty environment and exits at once
        let mut shell = std::process::Command::new("bash")
            .args(["-c", &format!("(env -i /bin/bash -c '{script}' &); sleep 30")])
            .env(JOB_ENV, &tag)
            .spawn()
            .unwrap();
        assert!(job_adopt(key, shell.id()));
        let (mut s, _) = l.accept().unwrap();
        let mut b = [0u8; 1];
        s.read_exact(&mut b).unwrap();
        let (peer, local) = (s.peer_addr().unwrap(), s.local_addr().unwrap());
        // the subshell has exited: the client hangs below this process now, not below the shell
        let mut pc = None;
        for _ in 0..50 {
            let p = resolve_peer(peer, local).expect("caller");
            if p.chain.get(1) == Some(&std::process::id()) {
                pc = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        let pc = pc.expect("re-parented to the subreaper");
        assert_eq!(pc.job, None);
        assert!(pc.orphan, "detached, untagged descendant must be restricted, not external: {pc:?}");
        let client = pc.chain[0];
        let _ = s.write_all(b"x");
        drop(s);
        let _ = shell.kill();
        let _ = shell.wait();
        // SAFETY: signal and reap the re-parented client (our child now).
        unsafe {
            libc::kill(client as libc::pid_t, libc::SIGKILL);
            let mut st = 0;
            libc::waitpid(client as libc::pid_t, &mut st, 0);
        }
        job_drop(key);
    }
}
