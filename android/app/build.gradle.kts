import java.io.File
import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins { id("com.android.application"); kotlin("android") }

abstract class BuildAndroidNoise @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection
    @get:Internal abstract val manifest: RegularFileProperty
    @get:Internal abstract val ndk: DirectoryProperty
    @get:Input abstract val ndkVersion: Property<String>
    @get:Input abstract val minSdk: Property<Int>
    @get:LocalState abstract val cargoDirectory: DirectoryProperty
    @get:OutputDirectory abstract val libraries: DirectoryProperty

    @TaskAction
    fun build() {
        val os = System.getProperty("os.name")
        val windows = os.startsWith("Windows")
        val host = when {
            windows -> "windows-x86_64"
            os.startsWith("Mac") -> "darwin-x86_64"
            else -> "linux-x86_64"
        }
        val clang = ndk.file("toolchains/llvm/prebuilt/$host/bin/clang" + if (windows) ".exe" else "").get().asFile
        check(clang.isFile) { "Install Android NDK ${ndkVersion.get()} for $host" }
        val targets = mapOf(
            "arm64-v8a" to "aarch64-linux-android",
            "armeabi-v7a" to "armv7-linux-androideabi",
            "x86_64" to "x86_64-linux-android",
            "x86" to "i686-linux-android",
        )
        val outputName = "libdesk_switch_noise.so"
        val targetDirectory = cargoDirectory.get().asFile
        targets.forEach { (abi, target) ->
            val clangTarget = if (abi == "armeabi-v7a") "armv7a-linux-androideabi" else target
            val cargoTarget = "CARGO_TARGET_" + target.replace('-', '_').uppercase()
            exec.exec {
                commandLine("cargo", "build", "--release", "--locked", "--jobs", "2",
                    "--manifest-path", manifest.get().asFile, "--target-dir", targetDirectory, "--target", target)
                environment("${cargoTarget}_LINKER", clang.absolutePath)
                environment("CARGO_ENCODED_RUSTFLAGS", "-Clink-arg=--target=$clangTarget${minSdk.get()}")
            }
            File(targetDirectory, "$target/release/$outputName")
                .copyTo(libraries.file("$abi/$outputName").get().asFile, overwrite = true)
        }
    }
}

val noiseNdkVersion = "28.2.13676358"
val appMinSdk = 26
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
    ndkVersion = noiseNdkVersion
    defaultConfig {
        applicationId = "io.github.deskswitch.remote.experimental"
        minSdk = appMinSdk
        targetSdk = 35
        versionCode = 7
        versionName = "0.3.4-experimental"
    }
    signingConfigs.getByName("debug") { storeFile = rootProject.file(".local/debug.keystore") }
    buildFeatures { buildConfig = true }
    buildTypes { release { isMinifyEnabled = false } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    testOptions { unitTests.isReturnDefaultValues = true }
    packaging { resources.excludes += "META-INF/DEPENDENCIES" }
}

val nativeNoise = rootProject.file("native")
val buildAndroidNoise by tasks.registering(BuildAndroidNoise::class) {
    manifest.set(nativeNoise.resolve("Cargo.toml"))
    sources.from(manifest, nativeNoise.resolve("Cargo.lock"), nativeNoise.resolve("src"), rootProject.file("../rust-toolchain.toml"))
    ndk.set(androidComponents.sdkComponents.ndkDirectory)
    ndkVersion.set(noiseNdkVersion)
    minSdk.set(appMinSdk)
    cargoDirectory.set(layout.buildDirectory.dir("noise/cargo"))
    libraries.set(layout.buildDirectory.dir("noise/jniLibs"))
}
androidComponents.onVariants { variant ->
    variant.sources.jniLibs?.addGeneratedSourceDirectory(buildAndroidNoise, BuildAndroidNoise::libraries)
}

dependencies {
    implementation(project(":core"))
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
