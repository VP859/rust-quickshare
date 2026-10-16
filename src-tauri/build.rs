fn main() {
    // Na Androidzie doklejamy `-llog`, ponieważ warstwa logowania (logcat) używa
    // `__android_log_print` z biblioteki `liblog.so`.
    #[cfg(target_os = "android")]
    println!("cargo:rustc-link-lib=log");

    // Oczekiwania cfg używane w kodzie (Tauri v2 ustawia `mobile` automatycznie
    // dla celów android/ios, ale deklarujemy to jawnie dla kontroli lintów).
    println!("cargo::rustc-check-cfg=cfg(mobile)");

    tauri_build::build();
}
