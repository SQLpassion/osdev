fn main()
{
    cc::Build::new()
        .file("src/libc.c")
        .flag("-mcmodel=large")
        .compile("interop");

    println!("cargo::rerun-if-changed=src/libc.c");
}