// Copyright 2021 Red Hat, Inc.
// SPDX-License-Identifier: Apache-2.0

use libc::{c_char, c_int};

pub const NET_FLAG_VFKIT: u32 = 1 << 0;

const NET_FEATURE_CSUM: u32 = 1 << 0;
const NET_FEATURE_GUEST_CSUM: u32 = 1 << 1;
const NET_FEATURE_GUEST_TSO4: u32 = 1 << 7;
const NET_FEATURE_GUEST_UFO: u32 = 1 << 10;
const NET_FEATURE_HOST_TSO4: u32 = 1 << 11;
const NET_FEATURE_HOST_UFO: u32 = 1 << 14;

pub const COMPAT_NET_FEATURES: u32 = NET_FEATURE_CSUM
    | NET_FEATURE_GUEST_CSUM
    | NET_FEATURE_GUEST_TSO4
    | NET_FEATURE_GUEST_UFO
    | NET_FEATURE_HOST_TSO4
    | NET_FEATURE_HOST_UFO;

#[link(name = "krun")]
extern "C" {
    pub fn krun_set_log_level(level: u32) -> i32;
    pub fn krun_create_ctx() -> i32;
    pub fn krun_free_ctx(ctx: u32) -> i32;
    pub fn krun_set_vm_config(ctx: u32, num_vcpus: u8, ram_mib: u32) -> i32;
    pub fn krun_set_root(ctx: u32, root_path: *const c_char) -> i32;
    pub fn krun_add_net_unixstream(
        ctx: u32,
        path: *const c_char,
        fd: c_int,
        mac: *mut u8,
        features: u32,
        flags: u32,
    ) -> i32;
    pub fn krun_add_net_unixgram(
        ctx: u32,
        path: *const c_char,
        fd: c_int,
        mac: *mut u8,
        features: u32,
        flags: u32,
    ) -> i32;
    pub fn krun_set_passt_fd(ctx: u32, fd: c_int) -> i32;
    pub fn krun_set_gvproxy_path(ctx: u32, path: *mut c_char) -> i32;
    pub fn krun_set_port_map(ctx: u32, port_map: *const *const c_char) -> i32;
    pub fn krun_set_workdir(ctx: u32, workdir_path: *const c_char) -> i32;
    pub fn krun_add_virtiofs(ctx: u32, tag: *const c_char, path: *const c_char) -> i32;
    pub fn krun_set_exec(
        ctx: u32,
        exec_path: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> i32;
    pub fn krun_set_env(ctx: u32, envp: *const *const c_char) -> i32;
    pub fn krun_start_enter(ctx: u32) -> i32;
}
