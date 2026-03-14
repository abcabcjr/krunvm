// Copyright 2021 Red Hat, Inc.
// SPDX-License-Identifier: Apache-2.0

use clap::Args;
use libc::{c_char, c_int};
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::fs;
use std::fs::File;
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::io::{Error, ErrorKind};
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "macos")]
use std::path::Path;

use crate::bindings;
use crate::utils::{mount_container, umount_container};
use crate::{
    normalize_optional_string, KrunvmConfig, NetworkBackend, NetworkTransport, VmConfig,
    VmNetworkConfig,
};

#[derive(Args, Debug)]
/// Start an existing microVM
pub struct StartCmd {
    /// Name of the microVM
    name: String,

    /// Command to run inside the VM
    command: Option<String>,

    /// Arguments to be passed to the command executed in the VM
    args: Vec<String>,

    /// Number of vCPUs
    #[arg(long)]
    cpus: Option<u8>, // TODO: implement or remove this

    /// Amount of RAM in MiB
    #[arg(long)]
    mem: Option<usize>, // TODO: implement or remove this

    /// env(s) in format "key=value" to be exposed to the VM
    #[arg(long = "env")]
    envs: Option<Vec<String>>,

    /// Networking backend to use for this run
    #[arg(long = "net-backend", value_enum)]
    net_backend: Option<NetworkBackend>,

    /// Unix socket path for the selected networking backend
    #[arg(long = "net-socket-path")]
    net_socket_path: Option<String>,

    /// Pre-opened file descriptor for the selected networking backend
    #[arg(long = "net-fd")]
    net_fd: Option<c_int>,
}

#[derive(Clone, Debug)]
struct ResolvedNetworkConfig {
    backend: NetworkBackend,
    socket_path: Option<String>,
    fd: Option<c_int>,
}

impl ResolvedNetworkConfig {
    fn resolve(
        vm_network: &VmNetworkConfig,
        backend_override: Option<NetworkBackend>,
        socket_path_override: Option<String>,
        fd_override: Option<c_int>,
    ) -> Result<ResolvedNetworkConfig, String> {
        let socket_path = if socket_path_override.is_some() {
            normalize_optional_string(socket_path_override)
        } else {
            vm_network.socket_path.clone()
        };

        let network = ResolvedNetworkConfig {
            backend: backend_override.unwrap_or(vm_network.backend),
            socket_path,
            fd: fd_override,
        };
        network.validate()?;
        Ok(network)
    }

    fn validate(&self) -> Result<(), String> {
        match self.backend {
            NetworkBackend::Tsi => {
                if self.socket_path.is_some() || self.fd.is_some() {
                    Err("TSI networking does not accept --net-socket-path or --net-fd.".to_string())
                } else {
                    Ok(())
                }
            }
            _ => {
                if self.socket_path.is_some() && self.fd.is_some() {
                    return Err(
                        "Networking backends accept either --net-socket-path or --net-fd, not both."
                            .to_string(),
                    );
                }
                if self.socket_path.is_none() && self.fd.is_none() {
                    return Err(format!(
                        "Networking backend '{}' requires --net-socket-path or --net-fd.",
                        self.backend
                    ));
                }
                Ok(())
            }
        }
    }

    fn validate_port_mapping(&self, vmcfg: &VmConfig) -> Result<(), String> {
        if self.backend == NetworkBackend::Passt && !vmcfg.mapped_ports.is_empty() {
            Err(
                "The passt backend does not support krunvm port mappings. Configure passt forwarding externally or remove --port mappings."
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }
}

impl StartCmd {
    pub fn run(self, cfg: &KrunvmConfig) {
        let StartCmd {
            name,
            command,
            args,
            cpus: _,
            mem: _,
            envs,
            net_backend,
            net_socket_path,
            net_fd,
        } = self;

        let vmcfg = match cfg.vmconfig_map.get(&name) {
            None => {
                println!("No VM found with name {}", name);
                std::process::exit(-1);
            }
            Some(vmcfg) => vmcfg,
        };

        umount_container(cfg, vmcfg).expect("Error unmounting container");
        let rootfs = mount_container(cfg, vmcfg).expect("Error mounting container");

        let network = match ResolvedNetworkConfig::resolve(
            &vmcfg.network,
            net_backend,
            net_socket_path,
            net_fd,
        ) {
            Ok(network) => network,
            Err(error) => {
                println!("{error}");
                std::process::exit(-1);
            }
        };
        if let Err(error) = network.validate_port_mapping(vmcfg) {
            println!("{error}");
            std::process::exit(-1);
        }

        let vm_args: Vec<CString> = if command.is_some() {
            args.into_iter()
                .map(|val| CString::new(val).unwrap())
                .collect()
        } else {
            Vec::new()
        };

        let env_pairs: Vec<CString> = if let Some(envs) = envs {
            envs.into_iter()
                .map(|val| CString::new(val).unwrap())
                .collect()
        } else {
            Vec::new()
        };

        set_rlimits();

        let _file = set_lock(&rootfs);

        unsafe {
            exec_vm(
                vmcfg,
                &rootfs,
                &network,
                command.as_deref(),
                vm_args,
                env_pairs,
            )
        };

        umount_container(cfg, vmcfg).expect("Error unmounting container");
    }
}

#[cfg(target_os = "linux")]
fn map_volumes(_ctx: u32, vmcfg: &VmConfig, rootfs: &str) {
    for (host_path, guest_path) in vmcfg.mapped_volumes.iter() {
        let host_dir = CString::new(host_path.to_string()).unwrap();
        let guest_dir = CString::new(format!("{}{}", rootfs, guest_path)).unwrap();

        let ret = unsafe { libc::mkdir(guest_dir.as_ptr(), 0o755) };
        if ret < 0 && Error::last_os_error().kind() != ErrorKind::AlreadyExists {
            println!("Error creating directory {:?}", guest_dir);
            std::process::exit(-1);
        }
        unsafe { libc::umount(guest_dir.as_ptr()) };
        let ret = unsafe {
            libc::mount(
                host_dir.as_ptr(),
                guest_dir.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            )
        };
        if ret < 0 {
            println!("Error mounting volume {}", guest_path);
            std::process::exit(-1);
        }
    }
}

#[cfg(target_os = "macos")]
fn map_volumes(ctx: u32, vmcfg: &VmConfig, rootfs: &str) -> Vec<(String, String)> {
    let mut mounts = Vec::new();
    for (idx, (host_path, guest_path)) in vmcfg.mapped_volumes.iter().enumerate() {
        let full_guest = format!("{}{}", &rootfs, guest_path);
        let full_guest_path = Path::new(&full_guest);
        if !full_guest_path.exists() {
            std::fs::create_dir(full_guest_path)
                .expect("Couldn't create guest_path for mapped volume");
        }
        let tag = format!("krunvm{}", idx);
        let c_tag = CString::new(tag.as_str()).unwrap();
        let c_host = CString::new(host_path.as_str()).unwrap();
        let ret = unsafe { bindings::krun_add_virtiofs(ctx, c_tag.as_ptr(), c_host.as_ptr()) };
        if ret < 0 {
            println!("Error setting VM mapped volume {}", guest_path);
            std::process::exit(-1);
        }
        mounts.push((tag, guest_path.to_string()));
    }
    mounts
}

unsafe fn exec_vm(
    vmcfg: &VmConfig,
    rootfs: &str,
    network: &ResolvedNetworkConfig,
    cmd: Option<&str>,
    args: Vec<CString>,
    env_pairs: Vec<CString>,
) {
    let log_ret = bindings::krun_set_log_level(5);
    if log_ret < 0 {
        eprintln!("Warning: failed to set libkrun log level: {}", log_ret);
    }

    let ctx = bindings::krun_create_ctx() as u32;

    let ret = bindings::krun_set_vm_config(ctx, vmcfg.cpus as u8, vmcfg.mem);
    if ret < 0 {
        println!("Error setting VM config: {}", ret);
        std::process::exit(-1);
    }

    let c_rootfs = CString::new(rootfs).unwrap();
    let ret = bindings::krun_set_root(ctx, c_rootfs.as_ptr());
    if ret < 0 {
        println!("Error setting VM rootfs: {}", ret);
        std::process::exit(-1);
    }

    configure_network(ctx, network);

    #[cfg(target_os = "linux")]
    map_volumes(ctx, vmcfg, rootfs);
    #[cfg(target_os = "macos")]
    let virtiofs_mounts = map_volumes(ctx, vmcfg, rootfs);
    #[cfg(target_os = "macos")]
    let mount_wrapper = build_mount_wrapper(rootfs, cmd, &vmcfg.workdir, &args, &virtiofs_mounts);

    let mut ports = Vec::new();
    for (host_port, guest_port) in vmcfg.mapped_ports.iter() {
        let map = format!("{}:{}", host_port, guest_port);
        ports.push(CString::new(map).unwrap());
    }
    let mut ps: Vec<*const c_char> = Vec::new();
    for port in ports.iter() {
        ps.push(port.as_ptr());
    }
    ps.push(std::ptr::null());

    if network.backend != NetworkBackend::Passt && !vmcfg.mapped_ports.is_empty() {
        let ret = bindings::krun_set_port_map(ctx, ps.as_ptr());
        if ret < 0 {
            println!("Error setting VM port map: {}", ret);
            std::process::exit(-1);
        }
    }

    if !vmcfg.workdir.is_empty() {
        let c_workdir = CString::new(vmcfg.workdir.clone()).unwrap();
        let ret = bindings::krun_set_workdir(ctx, c_workdir.as_ptr());
        if ret < 0 {
            println!("Error setting VM workdir: {}", ret);
            std::process::exit(-1);
        }
    }

    let hostname = CString::new(format!("HOSTNAME={}", vmcfg.name)).unwrap();
    let home = CString::new("HOME=/root").unwrap();

    let mut env: Vec<*const c_char> = Vec::new();
    env.push(hostname.as_ptr());
    env.push(home.as_ptr());
    for value in env_pairs.iter() {
        env.push(value.as_ptr());
    }
    env.push(std::ptr::null());

    #[cfg(target_os = "macos")]
    {
        if let Some((helper_path, helper_args)) = mount_wrapper {
            let mut argv: Vec<*const c_char> = helper_args.iter().map(|a| a.as_ptr()).collect();
            argv.push(std::ptr::null());
            let ret =
                bindings::krun_set_exec(ctx, helper_path.as_ptr(), argv.as_ptr(), env.as_ptr());
            if ret < 0 {
                println!("Error setting VM exec helper: {}", ret);
                std::process::exit(-1);
            }
        } else if let Some(cmd) = cmd {
            let mut argv: Vec<*const c_char> = Vec::new();
            for a in args.iter() {
                argv.push(a.as_ptr());
            }
            argv.push(std::ptr::null());

            let c_cmd = CString::new(cmd).unwrap();
            let ret = bindings::krun_set_exec(ctx, c_cmd.as_ptr(), argv.as_ptr(), env.as_ptr());
            if ret < 0 {
                println!("Error setting VM exec: {}", ret);
                std::process::exit(-1);
            }
        } else {
            let ret = bindings::krun_set_env(ctx, env.as_ptr());
            if ret < 0 {
                println!("Error setting VM environment variables: {}", ret);
                std::process::exit(-1);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(cmd) = cmd {
            let mut argv: Vec<*const c_char> = Vec::new();
            for a in args.iter() {
                argv.push(a.as_ptr());
            }
            argv.push(std::ptr::null());

            let c_cmd = CString::new(cmd).unwrap();
            let ret = bindings::krun_set_exec(ctx, c_cmd.as_ptr(), argv.as_ptr(), env.as_ptr());
            if ret < 0 {
                println!("Error setting VM config");
                std::process::exit(-1);
            }
        } else {
            let ret = bindings::krun_set_env(ctx, env.as_ptr());
            if ret < 0 {
                println!("Error setting VM environment variables");
                std::process::exit(-1);
            }
        }
    }

    let ret = bindings::krun_start_enter(ctx);
    if ret < 0 {
        println!("Error starting VM: {}", ret);
        std::process::exit(-1);
    }
}

unsafe fn configure_network(ctx: u32, network: &ResolvedNetworkConfig) {
    const DEFAULT_NET_MAC: [u8; 6] = [0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee];

    let socket_path = network
        .socket_path
        .as_ref()
        .map(|path| CString::new(path.as_str()).unwrap());
    let ret = match network.backend {
        NetworkBackend::Tsi => return,
        backend => {
            let Some(transport) = backend.transport() else {
                return;
            };
            let socket_path_ptr = socket_path
                .as_ref()
                .map(|path| path.as_ptr())
                .unwrap_or(std::ptr::null());
            let fd = network.fd.unwrap_or(-1);
            let mut mac = DEFAULT_NET_MAC;
            let flags = if backend == NetworkBackend::Gvproxy && socket_path.is_some() {
                bindings::NET_FLAG_VFKIT
            } else {
                0
            };

            match transport {
                NetworkTransport::UnixStream => bindings::krun_add_net_unixstream(
                    ctx,
                    socket_path_ptr,
                    fd,
                    mac.as_mut_ptr(),
                    bindings::COMPAT_NET_FEATURES,
                    flags,
                ),
                NetworkTransport::UnixGram => bindings::krun_add_net_unixgram(
                    ctx,
                    socket_path_ptr,
                    fd,
                    mac.as_mut_ptr(),
                    bindings::COMPAT_NET_FEATURES,
                    flags,
                ),
            }
        }
    };
    if ret < 0 {
        println!(
            "Error configuring VM network backend {}: {}",
            network.backend, ret
        );
        std::process::exit(-1);
    }
}

#[cfg(target_os = "macos")]
fn build_mount_wrapper(
    rootfs: &str,
    cmd: Option<&str>,
    workdir: &str,
    args: &[CString],
    mounts: &[(String, String)],
) -> Option<(CString, Vec<CString>)> {
    if mounts.is_empty() {
        return None;
    }

    let helper_path = write_mount_script(rootfs, workdir, mounts);

    let mut exec_args: Vec<CString> = Vec::new();
    let command = cmd.unwrap_or("/bin/sh");
    exec_args.push(CString::new(command).unwrap());
    exec_args.extend(args.iter().cloned());

    let helper_cstr = CString::new(helper_path).unwrap();
    Some((helper_cstr, exec_args))
}

#[cfg(target_os = "macos")]
fn write_mount_script(rootfs: &str, workdir: &str, mounts: &[(String, String)]) -> String {
    let host_path = format!("{}/.krunvm-mount.sh", rootfs);
    let guest_path = "/.krunvm-mount.sh".to_string();

    let mut file = File::create(&host_path).unwrap_or_else(|err| {
        println!("Error creating mount helper script: {}", err);
        std::process::exit(-1);
    });

    writeln!(file, "#!/bin/sh").unwrap();
    writeln!(file, "set -e").unwrap();
    for (tag, guest_path) in mounts {
        writeln!(file, "mount -t virtiofs {} {}", tag, guest_path).unwrap();
    }
    if !workdir.is_empty() {
        writeln!(file, "cd {}", workdir).unwrap();
    }
    writeln!(file, "exec \"$@\"").unwrap();

    let perms = fs::Permissions::from_mode(0o755);
    if let Err(err) = fs::set_permissions(&host_path, perms) {
        println!("Error setting mount helper permissions: {}", err);
        std::process::exit(-1);
    }

    guest_path
}

fn set_rlimits() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    let ret = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    if ret < 0 {
        panic!("Couldn't get RLIMIT_NOFILE value");
    }

    limit.rlim_cur = limit.rlim_max;
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
    if ret < 0 {
        panic!("Couldn't set RLIMIT_NOFILE value");
    }
}

fn set_lock(rootfs: &str) -> File {
    let lock_path = format!("{}/.krunvm.lock", rootfs);
    let file = File::create(lock_path).expect("Couldn't create lock file");

    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret < 0 {
        println!("Couldn't acquire lock file. Is another instance of this VM already running?");
        std::process::exit(-1);
    }

    file
}
