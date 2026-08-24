package com.onde.resilience;

import android.os.Bundle;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import androidx.appcompat.app.AppCompatActivity;

public class MainActivity extends AppCompatActivity {
    private WebView webView;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);

        webView = findViewById(R.id.webview);
        WebSettings webSettings = webView.getSettings();

        // Sécurité (Audit M6) : l'UI legacy (`assets/index.html`) est une
        // application monolithique en JavaScript vanilla inline, avec CSP
        // `connect-src 'none'` (aucune ressource externe). JavaScript est donc
        // REQUIS pour la navigation entre pages, la composition et les
        // interactions locales de cette page.
        //
        // En revanche, l'accès général aux fichiers (`file://`) et aux
        // ContentProviders (`content://`) est explicitement DÉSACTIVÉ : ces
        // deux flags permettraient à une page compromise de lire des fichiers
        // locaux ou des données d'autres applications. La page chargée via
        // `file:///android_asset/index.html` reste accessible sans eux — les
        // assets de l'APK sont toujours lisibles par la WebView qui les
        // charge, seul l'accès *arbitraire* aux autres fichiers est coupé.
        webSettings.setJavaScriptEnabled(true);
        webSettings.setDomStorageEnabled(true);
        webSettings.setDatabaseEnabled(true);
        webSettings.setAllowFileAccess(false);
        webSettings.setAllowContentAccess(false);
        webSettings.setUseWideViewPort(true);
        webSettings.setLoadWithOverviewMode(true);
        webSettings.setSupportZoom(true);

        // Testabilité E2E (T32-B / Midscene.js + Playwright via CDP) :
        // active le DevTools Protocol sur la WebView. Accessible uniquement en
        // local via `adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>`.
        // Dette T32-B : débogage WebView conditionné au build DEBUG ;
        // en release il reste DÉSACTIVÉ.
        if (BuildConfig.DEBUG) {
            webView.setWebContentsDebuggingEnabled(true);
        }

        webView.setWebViewClient(new WebViewClient());
        webView.loadUrl("file:///android_asset/index.html");
    }

    @Override
    public void onBackPressed() {
        if (webView.canGoBack()) {
            webView.goBack();
        } else {
            super.onBackPressed();
        }
    }
}