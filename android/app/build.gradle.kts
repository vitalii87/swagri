plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.swagri.android"
    compileSdk = 35

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "com.swagri.android"
        minSdk = 29
        targetSdk = 35
        versionCode = 15
        versionName = "0.14.1-alpha"

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            isDebuggable = false
        }
    }

    sourceSets["main"].jniLibs.srcDir("src/main/jniLibs")
}

val workspaceRoot = rootProject.projectDir.parentFile
val cargoExecutable = if (System.getProperty("os.name").startsWith("Windows")) "cargo.exe" else "cargo"

val buildRustArm64 by tasks.registering(Exec::class) {
    workingDir = workspaceRoot
    commandLine(
        cargoExecutable,
        "ndk",
        "-t",
        "arm64-v8a",
        "-o",
        project.file("src/main/jniLibs").absolutePath,
        "build",
        "--release",
        "-p",
        "swagri-agent",
        "--lib",
    )
}

tasks.named("preBuild").configure {
    dependsOn(buildRustArm64)
}
