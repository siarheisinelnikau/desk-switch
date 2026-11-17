plugins { kotlin("jvm") }
kotlin { jvmToolchain(23) }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
}
tasks.withType<JavaCompile>().configureEach { sourceCompatibility = "17"; targetCompatibility = "17" }
dependencies {
    implementation(project(":noise"))
    compileOnly("org.json:json:20250107")
    testImplementation("org.json:json:20250107")
    testImplementation("junit:junit:4.13.2")
}
tasks.test { testLogging { events("failed", "skipped") } }
