buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        // AGP 8.9.x => wymaga JDK 17 (kompatybilne z Gradle 8.14 / 9.x)
        classpath("com.android.tools.build:gradle:8.9.2")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.1.0")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}
