fn main() {
    // esp-hal's linker script; must come last.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}
