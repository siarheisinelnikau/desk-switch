import java.io.File
plugins { id("com.android.application"); kotlin("android") }
val prepareDebugKeystore by tasks.registering(Exec::class) {
    val destination = rootProject.file(".local/debug.keystore")
    outputs.file(destination)
    onlyIf { !destination.exists() }
    doFirst { destination.parentFile.mkdirs() }
    val executable = File(System.getProperty("java.home"), "bin/keytool" + if (System.getProperty("os.name").startsWith("Windows")) ".exe" else "")
    commandLine(executable.absolutePath, "-genkeypair", "-keystore", destination.absolutePath,
        "-storepass", "android", "-keypass", "android", "-alias", "androiddebugkey",
        "-dname", "CN=Desk Switch Experimental,O=Android,C=US", "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000", "-noprompt")
}
tasks.matching { it.name == "validateSigningDebug" }.configureEach { dependsOn(prepareDebugKeystore) }
android {
    namespace = "io.github.deskswitch.remote"
    compileSdk = 35
    defaultConfig {
        applicationId = "io.github.deskswitch.remote.experimental"
        minSdk = 26
        targetSdk = 35
        versionCode = 2
        versionName = "0.2.0-experimental"
    }
    signingConfigs.getByName("debug") { storeFile = rootProject.file(".local/debug.keystore") }
    buildFeatures { buildConfig = true }
    buildTypes { release { isMinifyEnabled = false } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    testOptions { unitTests.isReturnDefaultValues = true }
    packaging { resources.excludes += "META-INF/DEPENDENCIES" }
}
dependencies {
    implementation(project(":core"))
    implementation(project(":noise"))
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("androidx.annotation:annotation:1.9.1")
    implementation("androidx.lifecycle:lifecycle-viewmodel-ktx:2.6.2")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.6.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    testImplementation("junit:junit:4.13.2")
    constraints {
        implementation("androidx.annotation:annotation-experimental:1.4.1")
        implementation("androidx.collection:collection:1.1.0")
        implementation("androidx.concurrent:concurrent-futures:1.2.0")
        implementation("androidx.profileinstaller:profileinstaller:1.4.1")
        implementation("androidx.startup:startup-runtime:1.2.0")
    }
}
