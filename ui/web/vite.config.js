const { defineConfig } = require('vite');

// The UI is a static HTML file (ui/src/index.html) with inline JS — no bundling needed.
module.exports = defineConfig({
  root: '../src',
  build: {
    outDir: '../dist',
    emptyOutDir: true,
  },
});
