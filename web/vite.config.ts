import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue()],
  server: {
    // 端口刻意选在 49152-65535 动态段，避开 5173/3000 等常见开发端口。
    // 后端端口需与 src/config.ts 的 BACKEND_PORT 保持一致。
    port: 58173,
    // 端口被占用时直接报错，不要静默换成别的端口。
    strictPort: true,
    proxy: {
      // Frontend talks to the axum backend on the same origin via /api.
      '/api': {
        target: 'http://127.0.0.1:58081',
        changeOrigin: true,
      },
    },
  },
})
