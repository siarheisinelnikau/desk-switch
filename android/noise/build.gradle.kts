plugins { `java-library` }
java {
    sourceCompatibility = JavaVersion.VERSION_1_8
    targetCompatibility = JavaVersion.VERSION_1_8
}
tasks.processResources {
    from("LICENSE.txt") { into("licenses/noise-java") }
}
