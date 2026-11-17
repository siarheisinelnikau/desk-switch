plugins { kotlin("jvm") }
kotlin { jvmToolchain(23) }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
}
tasks.withType<JavaCompile>().configureEach { sourceCompatibility = "17"; targetCompatibility = "17" }
dependencies {
    compileOnly("org.json:json:20250107")
    testImplementation("org.json:json:20250107")
    testImplementation("junit:junit:4.13.2")
}
val nativeNoise = rootProject.file("native")
val hostCargo = layout.buildDirectory.dir("noise/host")
val hostLibrary = hostCargo.map { it.file("debug/${System.mapLibraryName("desk_switch_noise")}") }
val buildHostNoise by tasks.registering(Exec::class) {
    inputs.files(nativeNoise.resolve("Cargo.toml"), nativeNoise.resolve("Cargo.lock"), rootProject.file("../rust-toolchain.toml"))
    inputs.dir(nativeNoise.resolve("src"))
    outputs.file(hostLibrary)
    commandLine("cargo", "build", "--locked", "--jobs", "2",
        "--manifest-path", nativeNoise.resolve("Cargo.toml"), "--target-dir", hostCargo.get().asFile)
}
tasks.test {
    dependsOn(buildHostNoise)
    inputs.file(hostLibrary)
    val proof = providers.environmentVariable("DESK_SWITCH_MOBILE_PROOF")
    inputs.property("desktopInteropProof", proof.orElse(""))
    systemProperty("java.library.path", hostLibrary.get().asFile.parent)
    testLogging { events("failed", "skipped") }
}
