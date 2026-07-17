plugins {
    id("org.springframework.boot")
}

group = "com.example"
version = "1.0.0"

dependencies {
    // Pinned exact — will NOT match common-lib 1.4.2 for cascade
    implementation("com.example:common-lib:1.0.0")
}
