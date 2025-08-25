fn main() {
    println!("cargo::rustc-check-cfg=cfg(nightly)");
    let meta = rustc_version::version_meta().unwrap();
    if meta.channel == rustc_version::Channel::Nightly {
        println!("cargo:rustc-cfg=nightly");
    }
}
