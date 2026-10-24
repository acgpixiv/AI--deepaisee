import { createApp } from "vue";
import App from "./App.vue";
import "./styles.css";

// ---- 全局错误兜底：界面挂掉时把原因显示出来，而不是留一片黑屏。
// （Tauri 的 WebView 里没有控制台，出错时用户只能看到空白，极难排查）
function showFatal(title: string, detail: string) {
  const el = document.getElementById("app");
  if (!el) return;
  el.innerHTML = `
    <div style="padding:32px;font-family:system-ui,-apple-system,'Segoe UI',sans-serif;color:#dbe4f3">
      <div style="font-size:18px;font-weight:600;color:#ef4444;margin-bottom:10px">${title}</div>
      <pre style="white-space:pre-wrap;font-size:12px;line-height:1.6;background:#121826;
                  border:1px solid #24304a;border-radius:10px;padding:14px;max-height:70vh;
                  overflow:auto;color:#8ea0bf">${detail
                    .replace(/&/g, "&amp;")
                    .replace(/</g, "&lt;")
                    .replace(/>/g, "&gt;")}</pre>
    </div>`;
}

window.addEventListener("error", (e) => {
  showFatal("界面加载出错", `${e.message}\n${e.filename}:${e.lineno}:${e.colno}\n${e.error?.stack ?? ""}`);
});
window.addEventListener("unhandledrejection", (e) => {
  const r = e.reason as any;
  showFatal("未处理的异步错误", `${r?.message ?? String(r)}\n${r?.stack ?? ""}`);
});

try {
  createApp(App).mount("#app");
} catch (err: any) {
  showFatal("应用启动失败", `${err?.message ?? String(err)}\n${err?.stack ?? ""}`);
}

// 开发模式下把 api 挂到 window，方便用 WebView2 的 CDP 做界面自动化验收
// （正式构建不会执行，不影响包体与安全面）
if (import.meta.env.DEV) {
  import("./api").then((m) => {
    (window as any).__ds = m.api;
  });
}
