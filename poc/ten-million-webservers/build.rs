fn main() {
    let source = "src/network_dpdk_custom.c";
    println!("cargo::rerun-if-changed={source}");

    let flags = pkgconf::PkgConfigParser::run_pkg_config_cflags(["libdpdk"], None)
        .expect("read libdpdk compiler flags");
    cc::Build::new()
        .file(source)
        .flags(flags.split_whitespace())
        .compile("packetio");

    let dpdk = pkgconf::PkgConfigParser::new()
        .probe(["libdpdk"], None)
        .expect("find libdpdk");
    pkgconf::emit_cargo_metadata(&dpdk.libs, true);
}
