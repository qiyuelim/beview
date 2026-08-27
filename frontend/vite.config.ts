import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') },
  },
  server: {
    host: '0.0.0.0',
    port: 5173,
    watch: {
      // WSL2 下 inotify 文件事件不可靠，改为轮询 + 等待写完成，
      // 避免捕捉到半写入文件 / 漏事件导致的陈旧转换缓存（"does not provide export" 白屏）
      usePolling: true,
      interval: 300,
      awaitWriteFinish: { stabilityThreshold: 400, pollInterval: 100 },
    },
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8765',
        changeOrigin: true,
        configure(proxy) {
          proxy.on('error', (err) => console.error('[vite-proxy-error]', err))
          proxy.on('proxyReq', (_, req) => console.log('[proxyReq]', req.method, req.url))
          proxy.on('proxyRes', (p, req) => console.log('[proxyRes]', p.statusCode, req.url))
        },
      },
    },
  },
  build: {
    outDir: '../server/static',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        // v4.2 M8：包体 522KB 分包——vendor 独立成 chunk 便于缓存；页面本身按路由懒加载
        manualChunks: {
          'react-vendor': ['react', 'react-dom', 'react-router-dom'],
          phosphor: ['@phosphor-icons/react'],
          radix: ['radix-ui'],
          sonner: ['sonner'],
        },
      },
    },
  },
})
