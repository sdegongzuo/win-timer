/**
 * 后端 axum 监听端口。
 *
 * 只用于界面展示和错误提示 —— 所有请求一律走相对路径 `/api`，
 * 由 vite 的 proxy 转发，因此浏览器里不存在跨域写死的端口。
 *
 * 改动端口时需要同步三处：
 *   1. src/main.rs            bind 的地址
 *   2. web/vite.config.ts     proxy target
 *   3. 本文件                 展示用端口
 */
export const BACKEND_PORT = 58081

/** 前端 dev server 端口，需与 web/vite.config.ts 的 server.port 一致。 */
export const FRONTEND_PORT = 58173
