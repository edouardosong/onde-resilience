# AGENT : Développeur Android (Kotlin)

## Mission
Implémenter et maintenir l'app Android : UI Kotlin, intégration du core (via binding/FFI), état offline.

## Stack proposée (L'ACTEUR CHOISIT ses outils dans cette liste)

### Build
- [x] Gradle
- [x] Kotlin DSL
- [x] Android Gradle Plugin

### UI
- [x] Jetpack Compose
- [x] Material 3
- [x] ViewModel/StateFlow
- [x] Hilt/Koin DI

### Test unitaire
- [x] JUnit 5
- [x] Robolectric
- [x] MockK
- [x] Turbine (flow)

### Qualité
- [x] ktlint
- [x] detekt (lint)
- [x] Android Lint

## CHOIX EFFECTUÉ (déposé par le développeur Android)

### Build — Gradle + Kotlin DSL + AGP
- **Gradle 8.x + Kotlin DSL** (à la place du Groovy DSL actuel) : typé, composable, plus sûr à maintenir
  pour un build multi-module (core-ffi + app). Migration depuis les `.gradle` `app/build.gradle` Groovy actuels
  vers `build.gradle.kts` / `settings.gradle.kts`.
- **Android Gradle Plugin 8.x** (actuel 8.1.0, compileSdk 34) : on montant vers 8.5+ pour compatibilité
  Compose compiler plugin stabilité, sans casser minSdk 24.

### UI — Jetpack Compose + Material 3 + ViewModel/StateFlow
- **Jetpack Compose + Material 3** : remplace le WebView vanilla (`assets/index.html` + `MainActivity.java`).
  UI native déclarative, thème offline, adaptée à la nature statique-mesh.
- **ViewModel + StateFlow** : état offline observable immuable + UiState, réactivité idiomatique sans
  LiveData -> StateFlow pour les flux d'état mesh (statut noeud, messages, contacts).
- **Hilt (DI)** : injection au compile-time, standard Android, moins de boilerplate que Koin,
  indispensable pour injecter le binding Rust et le repository offline partout.

### Réseau — Ktor client (cocher le "si réseau")
- [x] **Ktor client** (si réseau) : moteur async léger (OkHttp engine) pour l'interface HTTP/transport
  vers core (DTN/nostr), testable avec `MockEngine`.

### Tests — JUnit 5 + Robolectric + MockK + Turbine
- **JUnit 5** : framework moderne pour les tests unitaires.
- **Robolectric** : exécution des tests Android (Compose/ViewModel) sur JVM sans émulateur.
- **MockK** : mocking Kotlin idiomatique (mocks léger pour core binding et repository).
- **Turbine** : assertions sur flows (StateFlow réactifs).

### Qualité — ktlint + detekt + Android Lint
- **ktlint** : formatage + lint Kotlin (syntaxe homogène).
- **detekt** : analyse statique (complexité, smells).
- **Android Lint** : gouvernance Android (API, perf, accessibilité).
- Gate qualité non-négociable : build + tests + ktlint + detekt + Android Lint tous verts avant fusion.

### Indispensables (cœur)
**Noyau :** Kotlin DSL + AGP + Jetpack Compose + Material 3 + ViewModel/StateFlow + Hilt + Ktor client + JUnit 5/Robolectric/MockK/Turbine + ktlint/detekt/Android Lint**.
C'est la stack minimale pour passer d'un shell WebView à une vraie app native Kotlin
avec intégration Rust, état offline et tests — le reste (Koin vs Hilt, Retrofit vs Ktor) est lectoriel.

## Repérage de structure (analyse du repo /home/linux/onde-resilience-clone/android)
- **État actuel : prototype legacy WebView vanilla.**
  - `MainActivity.java` (JAVA, pas Kotlin) héberge une WebView qui charge `assets/index.html`
    (41 Ko de HTML/JS vanilla inline, CSP `connect-src 'none'`). Navigation par `goBack()/goForward()`.
  - Build : GROOVY DSL — `settings.gradle` (AGP 8.1.0), `app/build.gradle` (namespace `com.onde.resilience`,
    compileSdk 34, minSdk 24, targetSdk 34, Java 8 source/target, versionName 0.2.0),
    `gradle-wrapper.properties` = Gradle 8.2, `dependencyLocking` activée sur compile/runtimeClasspath.
  - Dépendances actuelles : appcompat 1.6.1, material 1.9.0, webkit 1.7.0. **Aucune** :
    Kotlin, Compose, DI (Hilt/Koin), core-binding, tests (aucun dossier `test/`), outil de qualité.
  - Sécurité durcie existante : `network_security_config.xml` (cleartext=false, trust user CA désactivé),
    `usesCleartextTraffic=false`, permissions minimales (INTERNET, NETWORK/WIFI state/change), `allowBackup=false`.
  - Build : `build.sh` (ANDROID_HOME=/opt/android-sdk, gradle 8.2, `assembleRelease`).
  - Schéma cible à construire :
    ```
    app/
      src/main/java/com/onde/resilience/
        MainActivity.kt            (remplace .java, hôte Compose)
        ui/  (screens Compose, theme, components)
        vm/  (ViewModels + UiState, StateFlow)
        core/ (binding Rust via JNI/FFI, repository offline)
        di/  (Hilt modules)
      src/test/  (JUnit5+Robolectric+MockK+Turbine)
      src/androidTest/ (intégration)
    build.gradle.kts, settings.gradle.kts  (migration Kotlin DSL)
    ```
- Rôle dans la boucle : **maker** (j'implémente) — le checker/tester_android vérifie (JVM tests + Robolectric).

## Rôle dans la boucle
- maker / checker / arbitre / analyse — (voir procédures)
