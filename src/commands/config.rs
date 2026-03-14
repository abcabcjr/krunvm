// Copyright 2021 Red Hat, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::{store_krunvm_config, KrunvmConfig, NetworkBackend, VmNetworkConfig};
use clap::Args;

/// Configure global values
#[derive(Args, Debug)]
pub struct ConfigCmd {
    // Default number of vCPUs for newly created VMs
    #[arg(long)]
    cpus: Option<u32>,

    ///Default amount of RAM in MiB for newly created VMs
    #[arg(long)]
    mem: Option<u32>,

    /// DNS server to use in the microVM
    #[arg(long)]
    dns: Option<String>,

    /// Default networking backend for newly created VMs
    #[arg(long = "net-backend", value_enum)]
    net_backend: Option<NetworkBackend>,

    /// Default unix socket path for the selected networking backend
    #[arg(long = "net-socket-path")]
    net_socket_path: Option<String>,

    /// Remove the default unix socket path
    #[arg(long = "clear-net-socket-path")]
    clear_net_socket_path: bool,
}

impl ConfigCmd {
    pub fn run(self, cfg: &mut KrunvmConfig) {
        let mut cfg_changed = false;

        if let Some(cpus) = self.cpus {
            if cpus > 8 {
                println!("Error: the maximum number of CPUs supported is 8");
            } else {
                cfg.default_cpus = cpus;
                cfg_changed = true;
            }
        }

        if let Some(mem) = self.mem {
            if mem > 16384 {
                println!("Error: the maximum amount of RAM supported is 16384 MiB");
            } else {
                cfg.default_mem = mem;
                cfg_changed = true;
            }
        }

        if let Some(dns) = self.dns {
            cfg.default_dns = dns;
            cfg_changed = true;
        }

        let mut default_network = cfg.default_network.clone();
        if let Some(backend) = self.net_backend {
            default_network.backend = backend;
            if backend == NetworkBackend::Tsi {
                default_network.socket_path = None;
            }
            cfg_changed = true;
        }
        if self.clear_net_socket_path {
            default_network.socket_path = None;
            cfg_changed = true;
        }
        if self.net_socket_path.is_some() {
            default_network.socket_path = self.net_socket_path;
            cfg_changed = true;
        }
        default_network =
            VmNetworkConfig::new(default_network.backend, default_network.socket_path.clone());
        if let Err(error) = default_network.validate_persisted("Global network configuration") {
            println!("{error}");
            std::process::exit(-1);
        }
        cfg.default_network = default_network;

        if cfg_changed {
            store_krunvm_config(cfg);
        }

        println!("Global configuration:");
        println!(
            "Default number of CPUs for newly created VMs: {}",
            cfg.default_cpus
        );
        println!(
            "Default amount of RAM (MiB) for newly created VMs: {}",
            cfg.default_mem
        );
        println!(
            "Default DNS server for newly created VMs: {}",
            cfg.default_dns
        );
        println!(
            "Default network backend for newly created VMs: {}",
            cfg.default_network.backend
        );
        println!(
            "Default network socket path for newly created VMs: {}",
            cfg.default_network
                .socket_path
                .as_deref()
                .unwrap_or("<none>")
        );
    }
}
