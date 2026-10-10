fn main() {
    println!("cargo:rerun-if-changed=cfg.toml");
    embuild::espidf::sysenv::output();
}
