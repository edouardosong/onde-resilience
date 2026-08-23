# AGENT : Testeur Android E2E / QA Mobile

## Mission
Vérifier l'app Android sur émulateur/périphérique : fonctionnel, UI, régression visuelle, perf, crash. Choisit sa stack dans la liste.

## Stack proposée (L'ACTEUR CHOISIT ses outils dans cette liste)

### Niveau 1 — indispensable
- [ ] ADB
- [ ] Émulateur/AVD
- [ ] UI Automator / dump UI
- [ ] Logcat
- [ ] Screenshots / screenrecord

### Niveau 2 — fonctionnel
- [ ] Maestro
- [ ] Appium
- [ ] Espresso (si accès code)
- [ ] Monkey / App Crawler
- [ ] JUnit / Robolectric
- [ ] Proxy réseau (mitmproxy/Charles)

### Niveau 3 — debug avancé
- [ ] Perfetto / Profilers
- [ ] Bugreport / tombstones
- [ ] Crashlytics / Sentry
- [ ] APK Analyzer / JADX / Android Lint
- [ ] Visual regression / screenshot diff

> (rempli par l'agent — voir « ## CHOIX EFFECTUÉ » ci-dessous)

---

## CHOIX EFFECTUÉ — Testeur Android E2E / QA Mobile

> **Contrainte environnement détectée (factuelle)** : pas de `/dev/kvm`, aucun flag
> VT-x/SVM dans le CPU, aucune `system-images` installée, aucun device USB/réseau.
> → **Aucun émulateur/AVD ni périphérique exécutable dans la sandbox actuelle.**
> Les outils device-dépendants sont COCHÉS + configurés (commandes prêtes) mais
> signalés NON-EXÉCUTABLES ici — la collision entre la mission (vérifier sur device)
> et l'env (pas de device) est explicitement remontée au parent.
> Ce qui est réellement EXÉCUTÉ ici : build APK, lint, apkanalyzer/dexdump/aapt
> (analyse statique d'APK compilé), audit statique du rendu WebView (l'UI réelle).

### Niveau 1 — indispensable
- [x] **ADB** — ✅ présent (`/opt/android-sdk/platform-tools/adb` v37). Base de tout (install, logcat, screencap, input, ui dump). Indispensable.
- [x] **Émulateur/AVD** — ⚠️ COCHÉ + configuré (avdmanager présent, binaire emulator dans `/home/linux/Android/Sdk`) MAIS **non-exécutable** : pas de KVM ni system-image. Nécessaire pour tout test sur device.
- [x] **UI Automator / dump UI** — configuré via `adb shell uiautomator dump` + `adb exec-out uiautomator dump`. Indispensable pour valider la hiérarchie de vues. Non-exécutable sans device.
- [x] **Logcat** — configuré (`adb logcat`). Indispensable pour détecter crashes/ANR/webview console errors. Non-exécutable ici.
- [x] **Screenshots / screenrecord** — configuré (`adb exec-out screencap -p > x.png`, `screenrecord`). Base de la régression visuelle. Non-exécutable ici.

### Niveau 2 — fonctionnel
- [x] **Maestro** — COCHÉ : flux YAML déclaratif idéal pour tester la navigation à 6 tabs de la WebView. Nécessite device. Non-exécutable ici.
- [ ] **Appium** — non retenu (sur-engineering pour une app WebView monocouche vs Maestro).
- [x] **Espresso (accès au code)** — COCHÉ : accès au code (MainActivity.java). Pertinent pour tester l'activité native (chargement WebView). Nécessite un device/émulateur connecté pour réellement tourner. Non-exécutable ici.
- [x] **Monkey / App Crawler** — COCHÉ : `adb shell monkey` exploration aléatoire = excellent pour la robustesse/crashs de la WebView. Nécessite device. Non-exécutable ici.
- [x] **JUnit / Robolectric** — COCHÉ **+ réellement pertinent sans device** : Robolectric tourne sur JVM, peut tester que MainActivity charge bien index.html sans émulateur. Non encore présent dans le repo (à ajouter).
- [ ] **Proxy réseau (mitmproxy/Charles)** — non déployé : l'app est offline-first (CSP `connect-src 'none'`), pas de trafic réseau sortant depuis la WebView → outil sans objet pour ce shell.

### Niveau 3 — debug avancé
- [x] **Perfetto / Profilers** — décision : COCHÉ (choisi) mais non exécuté ici (nécessite device + aide ADB). La WebView est un simple shell de chargement local ; très peu d'intérêt de perf native.
- [x] **Bugreport / tombstones** — configuré (`adb bugreport`, `adb shell dumpsys dropbox`) pour collecter crash/tombstone sur device. Non-exécutable ici.
- [ ] **Crashlytics / Sentry** — non retenu (décision : pas d'outil de crash externalisé pour un prototype) (prototype ; gouvernement du code = logs local + crash du WebView). Document.
- [x] **APK Analyzer / JADX / Android Lint** — ✅ **EXÉCUTÉ ici** : `apkanalyzer`, `aapt`, `dexdump` (substitut JADX non installé) sur l'APK compilé ; `gradle lint` (0 erreur / 7 warnings). Indispensable à l'analyse statique sans device. JADX : non installé → `dexdump`/`apkanalyzer dex` en remplacement.
- [x] **Visual regression / screenshot diff** — COCHÉ : pipeline screenshot ADB + diff pixel. Source de référence = screenshots build précédent / rendu attendu. Non-exécutable sans device.

---

## Ce que ce testeur PEUT VÉRIFIER sur le projet (exploré, exécuté)

### ✅ Réellement exécuté dans cette sandbox
| Vérification | Outil | Résultat |
|---|---|---|
| Build debug APK | gradle assembleDebug | **SUCCESS** (18 s) |
| Build release APK | gradle assembleRelease | **SUCCESS** (19 s), lintVital OK |
| APK valide & signable | aapt badging | package `com.onde.resilience`, vCode 2 / vName 0.2.0, minSdk 24, target 34 |
| Permissions minimales | aapt/apkanalyzer | INTERNET, NETWORK_STATE, WIFI_STATE, CHANGE_WIFI_STATE (sans surplus) ✅ |
| Manifest / cicatrice d'export | aapt | 1 activité exportée (MainActivity LAUNCHER), rien d'autre exporté ✅ |
| Asset UI embarqué | apkanalyzer files | `/assets/index.html` présent ✅ |
| Lint statique | gradle lint | **0 erreur / 7 warnings** (voir détail) |
| Dex / classes | apkanalyzer dex | classes.dex 56700 refs, code réel compilé |
| Config sécurité réseau | audit XML | cleartext interdit + trust-anchors système ✅ |
| Rendu WebView (l'UI) | audit statique index.html | 6 pages : feed, radar, ai, wallet, wiki(p2p-encyclopedia) ; nav par tabs ; escape XSS `esc()` sur injection user ✅ |

### ⚠️ Configuré mais NON-EXÉCUTABLE (pas de device/émulateur dans la sandbox)
- Lancement sur device, UI Automator dump réel, logcat réel, screencap/screenrecord,
  Monkey, Maestro, Espresso (devrait tourner via connectedAndroidTest), Robolectric (à ajouter),
  Perfetto, bugreport/tombstones, visual diff end-to-end.

### 🔴 Findings statiques Android (à transmettre à l'équipe)
1. **OldTargetApi** — targetSdk 34 alors que API 36 disponible (`targetSdk 34`).
2. **GradleDependency ×3** — libs datées : appcompat 1.6.1→1.8.0, material 1.9.0→1.14.0, webkit 1.7.0→1.17.0.
3. **SetJavaScriptEnabled** — XSS review (mitigé par CSP `script-src 'unsafe-inline'` + `esc()` ; à confirmer sur device).
4. **DataExtractionRules** — `allowBackup` deprecated Android 12+ ; ajouter `dataExtractionRules`.
5. **UnusedResources** — `ic_launcher_background` inutilisé (l'icône est `res/Qr.xml`).

---

## Rôle dans la boucle
- maker / checker / arbitre / analyse — (voir procédures) 
