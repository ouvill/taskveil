import java.util.Properties
import org.gradle.api.GradleException

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

fun strictBooleanFlag(name: String, value: String?): Boolean? =
    value?.let {
        it.toBooleanStrictOrNull()
            ?: throw GradleException("$name must be either true or false")
    }

val taskveilStoreBuild =
    strictBooleanFlag(
        "Gradle property taskveilStoreBuild",
        providers.gradleProperty("taskveilStoreBuild").orNull,
    )
        ?: strictBooleanFlag(
            "TASKVEIL_ANDROID_STORE_BUILD",
            System.getenv("TASKVEIL_ANDROID_STORE_BUILD"),
        )
        ?: false

val taskveilKeyPropertiesFile = rootProject.file(
    providers.gradleProperty("taskveilKeyProperties").orNull
        ?: System.getenv("TASKVEIL_ANDROID_KEY_PROPERTIES")
        ?: "key.properties",
)
val taskveilKeyProperties = Properties()
if (taskveilStoreBuild && taskveilKeyPropertiesFile.isFile) {
    taskveilKeyPropertiesFile.inputStream().use(taskveilKeyProperties::load)
}

fun taskveilSigningValue(environmentName: String, propertyName: String): String? =
    System.getenv(environmentName)?.trim()?.takeIf(String::isNotEmpty)
        ?: taskveilKeyProperties.getProperty(propertyName)?.trim()?.takeIf(String::isNotEmpty)

val taskveilStoreFilePath = if (taskveilStoreBuild) {
    taskveilSigningValue("TASKVEIL_ANDROID_KEYSTORE_PATH", "storeFile")
} else {
    null
}
val taskveilStorePassword = if (taskveilStoreBuild) {
    taskveilSigningValue("TASKVEIL_ANDROID_KEYSTORE_PASSWORD", "storePassword")
} else {
    null
}
val taskveilKeyAlias = if (taskveilStoreBuild) {
    taskveilSigningValue("TASKVEIL_ANDROID_KEY_ALIAS", "keyAlias")
} else {
    null
}
val taskveilKeyPassword = if (taskveilStoreBuild) {
    taskveilSigningValue("TASKVEIL_ANDROID_KEY_PASSWORD", "keyPassword")
} else {
    null
}
val taskveilMissingSigningValues = if (taskveilStoreBuild) {
    listOfNotNull(
        "TASKVEIL_ANDROID_KEYSTORE_PATH/storeFile".takeIf {
            taskveilStoreFilePath == null
        },
        "TASKVEIL_ANDROID_KEYSTORE_PASSWORD/storePassword".takeIf {
            taskveilStorePassword == null
        },
        "TASKVEIL_ANDROID_KEY_ALIAS/keyAlias".takeIf {
            taskveilKeyAlias == null
        },
        "TASKVEIL_ANDROID_KEY_PASSWORD/keyPassword".takeIf {
            taskveilKeyPassword == null
        },
    )
} else {
    emptyList()
}
if (taskveilMissingSigningValues.isNotEmpty()) {
    throw GradleException(
        "Store release signing requires all configured values; missing: " +
            taskveilMissingSigningValues.joinToString(),
    )
}
val taskveilStoreFile = taskveilStoreFilePath?.let(rootProject::file)
if (taskveilStoreBuild && taskveilStoreFile?.isFile != true) {
    throw GradleException("Store release signing keystore file does not exist")
}

android {
    namespace = "com.taskveil.app"
    compileSdk = 36
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
        isCoreLibraryDesugaringEnabled = true
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.taskveil.app"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = 35
        versionCode = flutter.versionCode
        versionName = flutter.versionName
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    signingConfigs {
        if (taskveilStoreBuild) {
            create("taskveilStore") {
                storeFile = requireNotNull(taskveilStoreFile)
                storePassword = requireNotNull(taskveilStorePassword)
                keyAlias = requireNotNull(taskveilKeyAlias)
                keyPassword = requireNotNull(taskveilKeyPassword)
            }
        }
    }

    buildTypes {
        release {
            // An ordinary release build is intentionally unsigned and is only a
            // package/JNI validation artifact. Store signing is opt-in so a
            // missing production keystore can never fall back to the debug key.
            signingConfig = if (taskveilStoreBuild) {
                signingConfigs.getByName("taskveilStore")
            } else {
                null
            }
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    val androidXTestJunitVersion = "1.2.1"
    val androidXTestRunnerVersion = "1.6.2"
    val androidXTestRulesVersion = "1.6.1"
    val espressoVersion = "3.6.1"

    // Flutter 3.44.6 integration_test exposes dynamic AndroidX Test dependencies
    // on the debug runtime. Keep that graph aligned with the instrumentation APK.
    // Re-evaluate these constraints when Flutter stops exposing dynamic versions.
    constraints {
        add("debugImplementation", "androidx.test:runner:$androidXTestRunnerVersion") {
            because("Flutter integration_test and androidTest runtimes must resolve the same runner")
        }
        add("debugImplementation", "androidx.test:rules:$androidXTestRulesVersion") {
            because("Flutter integration_test exposes AndroidX Test rules on the debug runtime")
        }
        add("debugImplementation", "androidx.test.espresso:espresso-core:$espressoVersion") {
            because("Flutter integration_test exposes Espresso on the debug runtime")
        }
    }

    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.4")
    androidTestImplementation("androidx.test.ext:junit:$androidXTestJunitVersion")
    androidTestImplementation("androidx.test:runner:$androidXTestRunnerVersion")
}
