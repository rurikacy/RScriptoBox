import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ['**/src-tauri/**', '**/dist/**', '**/.local-test/**', '**/release/**', '**/tests/runner/target/**'] } },
  build: { target: 'es2022' },
});
