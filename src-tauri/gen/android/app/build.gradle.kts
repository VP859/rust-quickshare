// =============================================================================
//  RQS Quick Share – app/build.gradle.kts
//  Wersja dostosowana do Androida (nakładka tworzona przez `cargo tauri android init`).
//  Pliki wygenerowane automatycznie zostają w .gitignore; ten plik jest
//  wersjonowany, ponieważ zawiera ręczne zmiany:
//    * namespace/applicationId = com.rqs.quickshare (zgodnie z tauri.conf.json)
//    * minSdk 24 / targetSdk 35
//    * TAURI_KOTLIN_CLASS -> nasza MainActivity (obsługa Share Sheet)
//    * dependencje: androidx.core (FileProvider), accompanist/permission nie są
//      potrzebne – uprawnienia prosimy po stronie Rust/JNI.
// =============================================================================
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Wersja aplikacji pobierana z tauri.conf.json (single source of truth).
val appVersion: String by lazy {
    val cfg = file("$rootDir/../../tauri.conf.json")
    if (cfg.exists()) {
        Regex("\"version\"\\s*:\\s*\"([^\"]+)\"").find(cfg.readText())?.groupValues?.get(1) ?: "0.1.0"
    } else "0.1.0"
}

android {
    namespace = "com.rqs.quickshare"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.rqs.quickshare"
        // Iroh + QUIC wymagają nowszych API; 24 to bezpieczne minimum (Android 7.0)
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = appVersion

        // Tauri v2 nie używa placeholdera w manifeście – klasa Kotlin jest
        // wskazywana przez <meta-data android:name="TAURI_KOTLIN_CLASS">
        // bezpośrednio w AndroidManifest.xml (com.rqs.quickshare.MainActivity).

        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    buildTypes {
        getByName("release") {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }

    sourceSets {
        getByName("main") {
            manifest.srcFile("src/main/AndroidManifest.xml")
            java.srcDirs("src/main/java")
            // Tutaj `cargo tauri android build` podkłada librqs_gui_lib.so
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    lint {
        // Iroh wymaga INTERNET/WAKE_LOCK – lint nie może ich wywalać jako błędu
        abortOnError = false
        disable += setOf("ExpiredTargetSdkVersion", "NotificationPermission")
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.activity:activity-ktx:1.9.2")
    implementation("androidx.lifecycle:lifecycle-service:2.8.6")
    implementation("androidx.core:core-splashscreen:1.0.1")
    testImplementation("junit:junit:4.13.2")
}

// -----------------------------------------------------------------------------
// Zadanie pomocnicze: przed assemblingiem kopiujemy bibliotekę .so z Cargo do
// src/main/jniLibs/<abi>/librqs_gui_lib.so  (robione też przez `tauri android build`,
// tutaj tylko dla wygodnego `./gradlew assembleDebug` podczas debugowania).
// -----------------------------------------------------------------------------
tasks.register("copyRustLib") {
    doLast {
        val outRoot = file("$rootDir/../target")
        val mapping = mapOf(
            "arm64-v8a" to "aarch64-linux-android",
            "armeabi-v7a" to "armv7-linux-androideabi",
            "x86_64" to "x86_64-linux-android",
        )
        mapping.forEach { (abi, triple) ->
            val src = file("$outRoot/$triple/debug/librqs_gui_lib.so")
            if (src.exists()) {
                val dstDir = file("src/main/jniLibs/$abi")
                dstDir.mkdirs()
                copy { from(src); into(dstDir); rename { "librqs_gui_lib.so" } }
            }
        }
    }
}
// W trybie CI (`cargo tauri android build`) zadanie jest pomijane, bo Tauri
// sam kopiuje artefakty Cargo do jniLibs. Uruchamiamy je tylko lokalnie.
if (System.getenv("CI") == null) {
    tasks.matching { it.name.startsWith("merge") && it.name.contains("JniLibFolders") }
        .configureEach { dependsOn("copyRustLib") }
}
