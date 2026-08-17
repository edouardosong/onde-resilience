plugins {
    `kotlin-dsl`
}

gradlePlugin {
    plugins {
        create("pluginsForCoolKids") {
            id = "rust"
            implementationClass = "RustPlugin"
        }
    }
}

repositories {
    google()
    mavenCentral()
}

dependencies {
    compileOnly(gradleApi())
    implementation("com.android.tools.build:gradle:8.11.0")
}

configurations.configureEach {
    if (name == "compileClasspath" || name == "runtimeClasspath"
            || name.endsWith("CompileClasspath") || name.endsWith("RuntimeClasspath")) {
        resolutionStrategy.activateDependencyLocking()
    }
}

