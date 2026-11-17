import java.util.Properties

plugins {
    id("com.android.application")
}

android {
    namespace = "io.github.slashneck.tether"
    compileSdk = 37

    defaultConfig {
        applicationId = "io.github.slashneck.tether"
        minSdk = 26
        targetSdk = 35
        versionCode = 10001
        versionName = "1.0.1"
    }

    // The release key lives outside the repo: %USERPROFILE%\.tether-keys\keystore.properties (or the path in
    // TETHER_KEYSTORE). Without it, release builds are signed with the debug key so anyone can still build.
    val keyProps = Properties().apply {
        val path = System.getenv("TETHER_KEYSTORE")
            ?: "${System.getProperty("user.home")}/.tether-keys/keystore.properties"
        val f = file(path)
        if (f.isFile) f.inputStream().use { load(it) }
    }
    signingConfigs {
        if (keyProps.containsKey("storeFile")) {
            create("release") {
                storeFile = file(keyProps.getProperty("storeFile"))
                storePassword = keyProps.getProperty("storePassword")
                keyAlias = keyProps.getProperty("keyAlias")
                keyPassword = keyProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    buildFeatures {
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
