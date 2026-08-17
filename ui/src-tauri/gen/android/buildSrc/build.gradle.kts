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

// Forcer les versions corrigées des dépendances transitives vulnérables
// (Aikido/Snyk : bcprov certificate validation, bcpkix signature forgery,
// bcutil/netty/commons-compress DoS, jdom2 XXE). Les versions sont imposées
// à la résolution, y compris quand le lockfile contient l'ancienne version.
configurations.configureEach {
    resolutionStrategy {
        force(
            "org.bouncycastle:bcprov-jdk18on:1.80",
            "org.bouncycastle:bcutil-jdk18on:1.80",
            "org.bouncycastle:bcpkix-jdk18on:1.80",
            "io.netty:netty-buffer:4.1.118.Final",
            "io.netty:netty-codec:4.1.118.Final",
            "io.netty:netty-codec-http:4.1.118.Final",
            "io.netty:netty-codec-http2:4.1.118.Final",
            "io.netty:netty-codec-socks:4.1.118.Final",
            "io.netty:netty-common:4.1.118.Final",
            "io.netty:netty-handler:4.1.118.Final",
            "io.netty:netty-handler-proxy:4.1.118.Final",
            "io.netty:netty-resolver:4.1.118.Final",
            "io.netty:netty-transport:4.1.118.Final",
            "io.netty:netty-transport-native-unix-common:4.1.118.Final",
            "org.apache.commons:commons-compress:1.27.1",
            "org.jdom:jdom2:2.0.6.2"
        )
    }
}

configurations.configureEach {
    if (name == "compileClasspath" || name == "runtimeClasspath"
            || name.endsWith("CompileClasspath") || name.endsWith("RuntimeClasspath")) {
        resolutionStrategy.activateDependencyLocking()
    }
}

