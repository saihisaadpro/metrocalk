fn main() {
    println!("cargo:rerun-if-changed=native/xatlas_bridge.cpp");
    println!("cargo:rerun-if-changed=vendor/xatlas.cpp");
    println!("cargo:rerun-if-changed=vendor/xatlas.h");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .warnings(false)
        .include("vendor")
        .file("vendor/xatlas.cpp")
        .file("native/xatlas_bridge.cpp")
        .flag_if_supported("-std=c++14")
        .flag_if_supported("/std:c++14")
        .compile("metrocalk_xatlas");
}
