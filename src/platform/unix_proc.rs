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
//! * **Session "job":** there are no Job Objects. Each session gets a random tag in its environment
//!   ([`JOB_ENV`]); every descendant inherits it, also detached ones (`setsid`, `nohup`, double fork – they are
//!   re-parented to init or a subreaper, so the process tree alone would lose them). The hub reads the initial
//!   environment of the caller (`/proc/<pid>/environ`, macOS `KERN_PROCARGS2`) and the process tree; the tree wins: a
//!   caller below the shell of a session belongs to that session even if its environment names another one.
//!   A process that clears its environment *and* detaches is treated as an external client (like a process outside
//!   every Job Object on Windows). As on Windows this is no boundary against hostile code of the same user (it can
//!   read other environments and forge the tag); the chain rules protect against steered agents, not local malware.
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
}

/// Snapshot of all processes: PID → stat.
fn snapshot() -> HashMap<u32, Stat> {
    sys::all_pids().into_iter().filter_map(|p| Some((p, sys::stat(p)?))).collect()
}

/// Is `pid` (with start `start`) the shell or a descendant of the shell `shell` (PID, start)? Every parent must have
/// started no later than its child (otherwise its PID was reused).
fn below(snap: &HashMap<u32, Stat>, pid: u32, shell: (u32, u64)) -> bool {
    let (mut cur, mut cur_start) = (pid, snap.get(&pid).map(|s| s.start).unwrap_or(u64::MAX));
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

/// Running (non-zombie) processes of the job: the shell, its descendants and every process with the job's tag.
pub fn job_pids(key: u64) -> Option<Vec<u32>> {
    let (tag, shell) = {
        let g = jobs();
        let j = g.iter().find(|j| j.key == key)?;
        (j.tag.clone(), j.shell)
    };
    let snap = snapshot();
    if snap.is_empty() {
        return None;
    }
    let me = std::process::id();
    let mut out: Vec<u32> = snap
        .iter()
        .filter(|(p, s)| **p != me && !s.zombie)
        .filter(|(p, _)| shell.is_some_and(|sh| below(&snap, **p, sh)) || sys::environ(**p).and_then(|e| env_value(&e, JOB_ENV)).is_some_and(|t| t == tag))
        .map(|(p, _)| *p)
        .collect();
    out.sort_unstable();
    Some(out)
}

fn kill(pid: u32) -> bool {
    // SAFETY: plain system call.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
}

pub fn job_terminate_all(key: u64) -> bool {
    let Some(pids) = job_pids(key) else { return false };
    for p in pids {
        kill(p);
    }
    true
}

pub fn job_terminate_listed(key: u64, procs: &[(u32, u64)]) -> usize {
    let Some(pids) = job_pids(key) else { return 0 };
    procs.iter().filter(|(p, born)| pids.contains(p) && process_born(*p) == Some(*born) && kill(*p)).count()
}

// ---------------------------------------------------------------------------------------------------------------------
// Public functions used by `process`

pub fn process_born(pid: u32) -> Option<u64> {
    sys::stat(pid).filter(|s| !s.zombie).map(|s| s.start)
}

/// Owner of the client socket `peer` → `local`. Several processes can hold the same socket (inherited descriptor):
/// then the one started last (a child that inherited it from its parent) is taken – it is the one that can still use
/// it after the parent hands over. `None` if there is no owner or only this process (outside the tests).
pub fn tcp_peer_pid(peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    let (SocketAddr::V4(peer), SocketAddr::V4(local)) = (peer, local) else { return None };
    let owners = sys::socket_owners(peer, local);
    let me = std::process::id();
    owners
        .into_iter()
        .filter(|p| *p != me || cfg!(test))
        .filter_map(|p| Some((p, process_born(p)?)))
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
}

/// Caller of an accepted connection (see the module text). `None` = refuse.
pub fn resolve_peer(peer: SocketAddr, local: SocketAddr) -> Option<PeerChain> {
    let pid = tcp_peer_pid(peer, local)?;
    let born = process_born(pid)?;
    // Second look: the socket still belongs to the same process (same PID, same start time).
    if tcp_peer_pid(peer, local) != Some(pid) || process_born(pid) != Some(born) {
        return None;
    }
    let snap = snapshot();
    let chain = walk(&snap, pid, born);
    let registered: Vec<(u64, String, Option<(u32, u64)>)> = jobs().iter().map(|j| (j.key, j.tag.clone(), j.shell)).collect();
    // 1. Tree: below the shell of a session.
    let by_tree = registered.iter().find(|(_, _, sh)| sh.is_some_and(|sh| below(&snap, pid, sh))).map(|(k, ..)| *k);
    // 2. Environment tag (detached descendants).
    let job = by_tree.or_else(|| {
        let tag = env_value(&sys::environ(pid)?, JOB_ENV)?;
        registered.iter().find(|(_, t, _)| *t == tag).map(|(k, ..)| *k)
    });
    Some(PeerChain { chain, orphan: false, job })
}

/// Ancestors of `pid` (start `born`), nearest first, each started no later than its child.
fn walk(snap: &HashMap<u32, Stat>, pid: u32, born: u64) -> Vec<u32> {
    let mut chain = vec![pid];
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
        // The shell carries `tag`; its child claims the OTHER job's tag – the tree wins.
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
        // Without the shell registered (e.g. it has gone), the tag decides.
        job_drop(key);
        let pc = resolve_peer(s.peer_addr().unwrap(), s.local_addr().unwrap()).expect("caller");
        assert_eq!(pc.job, Some(other));
        let _ = s.write_all(b"x");
        job_terminate_all(other);
        let _ = shell.kill();
        let _ = shell.wait();
        job_drop(other);
    }
}
