# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile
# Classes and methods called only from Rust through JNI (src-tauri/src/platform.rs,
# looked up by name). Nothing in Kotlin calls them, so without these rules R8
# removes or renames them in release builds and the calls fail at run time.
# Keep this list in sync with the `jni_str!("io.github.vp859.ferry.…")` calls.
-keep class io.github.vp859.ferry.FileOpener { *; }
-keep class io.github.vp859.ferry.ShareTargets { *; }
-keep class io.github.vp859.ferry.Restarter { *; }
-keep class io.github.vp859.ferry.FerryPermissions { *; }
-keep class io.github.vp859.ferry.PowerHelper { *; }
-keep class io.github.vp859.ferry.FerryService { *; }
-keep class io.github.vp859.ferry.FerryService$Companion { *; }
