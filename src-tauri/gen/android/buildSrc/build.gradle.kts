plugins {
    `kotlin-dsl` // NIE dodawaj tu `kotlin("jvm")` – kotlin-dsl zarządza kompilatorem samodzielnie
}

repositories {
    mavenCentral()
    google()
}

dependencies {
    implementation("com.android.tools.build:gradle:8.9.2")
}
