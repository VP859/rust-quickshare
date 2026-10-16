plugins {
    `kotlin-dsl` // NIE dodawaj tu `kotlin("jvm")` – kotlin-dsl zarządza kompilatorem samodzielnie
}

repositories {
    mavenCentral()
    google()
}

dependencies {
    implementation("com.android.tools.build:gradle:8.9.2")
    // app/build.gradle.kts używa writeAsString() z tego API – bez tej zależności
    // buildSrc nie skompiluje się (Unresolved reference).
    implementation("org.jetbrains.kotlin:kotlin-stdlib")
}
