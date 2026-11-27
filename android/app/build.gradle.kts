import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

val moduleProperties = Properties().apply {
    rootProject.file("../ksu-module/module.prop").inputStream().use(::load)
}

android {
    namespace = "ch.ausfuhrli.bareboot"
    compileSdk = 37

    defaultConfig {
        applicationId = "ch.ausfuhrli.bareboot"
        minSdk = 30
        targetSdk = 36
        versionCode = moduleProperties.getProperty("versionCode").toInt()
        versionName = moduleProperties.getProperty("version")

        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    signingConfigs {
        val keystore = System.getenv("BAREBOOT_KEYSTORE")
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("BAREBOOT_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("BAREBOOT_KEY_ALIAS")
                keyPassword = System.getenv("BAREBOOT_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

kotlin {
    compilerOptions {
        optIn.addAll(
            "androidx.compose.material3.ExperimentalMaterial3Api",
            "androidx.compose.material3.ExperimentalMaterial3ExpressiveApi",
        )
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.11.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.1")

    implementation("io.github.sagernet:libghostty-android-compose:0.1.0-alpha02")
    implementation("io.github.sagernet:libghostty-android-extras:0.1.0-alpha02")
}
