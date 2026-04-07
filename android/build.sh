#!/bin/bash
export ANDROID_HOME=/opt/android-sdk
export PATH=$PATH:/opt/android-sdk/cmdline-tools/latest/bin:/opt/android-sdk/platform-tools

if [ ! -d /opt/gradle-8.2 ]; then
    curl -sL https://services.gradle.org/distributions/gradle-8.2-bin.zip -o /tmp/gradle.zip
    unzip -q /tmp/gradle.zip -d /opt
fi

export PATH=$PATH:/opt/gradle-8.2/bin
cd /workspace/android
gradle assembleRelease 2>&1
echo "BUILD_RESULT=$?"
ls -la app/build/outputs/apk/release/*.apk 2>/dev/null || echo "No release APK found"