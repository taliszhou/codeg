import { detectEnvironment } from "./detect"
import type { RemoteTransportConfig, Transport } from "./types"

export type { RemoteTransportConfig, Transport, UnsubscribeFn } from "./types"

let _shellTransport: Transport | null = null
let _remoteTransport: Transport | null = null
let _remoteConfig: RemoteTransportConfig | null = null

function createTauriTransport(): Transport {
  // Use dynamic require to avoid bundling tauri deps in web mode.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { TauriTransport } = require("./tauri-transport") as {
    TauriTransport: new () => Transport
  }
  return new TauriTransport()
}

function createWebTransport(baseUrl: string): Transport {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { WebTransport } = require("./web-transport") as {
    WebTransport: new (baseUrl: string) => Transport
  }
  return new WebTransport(baseUrl)
}

export function getShellTransport(): Transport {
  if (!_shellTransport) {
    const env = detectEnvironment()
    _shellTransport =
      env === "tauri"
        ? createTauriTransport()
        : createWebTransport(window.location.origin)
  }
  return _shellTransport
}

export function configureRemoteDesktopTransport(
  config: RemoteTransportConfig
): void {
  _remoteTransport?.destroy?.()
  _remoteConfig = config
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { RemoteDesktopTransport } = require("./remote-desktop-transport") as {
    RemoteDesktopTransport: new (config: RemoteTransportConfig) => Transport
  }
  _remoteTransport = new RemoteDesktopTransport(config)
}

export function clearRemoteDesktopTransport(): void {
  _remoteTransport?.destroy?.()
  _remoteTransport = null
  _remoteConfig = null
}

export function getActiveRemoteConnectionId(): number | null {
  return _remoteConfig?.id ?? null
}

export function getTransport(): Transport {
  // codeg: 优先跟 active device 走 (workspace 路由切换到远端设备时),
  // 否则回退上游 remote-desktop / 本机 shell。
  const deviceId = getActiveDeviceId()
  if (deviceId != null) return getRemoteTransport(deviceId)
  return _remoteTransport ?? getShellTransport()
}

/** codeg: 强制拿本机 transport, 忽略当前 active device — 用于固定本机的 API
 * (如远程设备 CRUD 本身、本地 NPU)。*/
export function getLocalTransport(): Transport {
  return getShellTransport()
}

// ===== codeg: 多设备 active-device 切换 (localStorage + /api/remote/{id}/ 代理) =====
const ACTIVE_DEVICE_KEY = "codeg_active_device_id"
const ACTIVE_DEVICE_EVENT = "codeg:active-device-changed"
const _deviceTransports = new Map<number, Transport>()

function buildDeviceTransport(deviceId: number): Transport {
  const cached = _deviceTransports.get(deviceId)
  if (cached) return cached
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { RemoteWebTransport } = require("./remote-web-transport") as {
    RemoteWebTransport: new (baseUrl: string, deviceId: number) => Transport
  }
  const t = new RemoteWebTransport(window.location.origin, deviceId)
  _deviceTransports.set(deviceId, t)
  return t
}

/** 读当前 active device id (仅 /workspace 路由生效, 其它路径恒走本机)。*/
export function getActiveDeviceId(): number | null {
  if (typeof window === "undefined") return null
  if (!window.location.pathname.startsWith("/workspace")) return null
  const raw = window.localStorage.getItem(ACTIVE_DEVICE_KEY)
  if (!raw) return null
  const n = parseInt(raw, 10)
  return isNaN(n) || n < 1 ? null : n
}

/** 设置 active device (null = 本机)。广播事件让组件刷新数据。*/
export function setActiveDeviceId(deviceId: number | null) {
  if (typeof window === "undefined") return
  if (deviceId == null) {
    window.localStorage.removeItem(ACTIVE_DEVICE_KEY)
  } else {
    window.localStorage.setItem(ACTIVE_DEVICE_KEY, String(deviceId))
  }
  window.dispatchEvent(new CustomEvent(ACTIVE_DEVICE_EVENT, { detail: { deviceId } }))
}

/** 订阅 active device 变化, 返回取消函数。*/
export function subscribeActiveDevice(
  handler: (deviceId: number | null) => void
): () => void {
  if (typeof window === "undefined") return () => {}
  const listener = (e: Event) => {
    const detail = (e as CustomEvent).detail as { deviceId: number | null }
    handler(detail?.deviceId ?? null)
  }
  window.addEventListener(ACTIVE_DEVICE_EVENT, listener)
  return () => window.removeEventListener(ACTIVE_DEVICE_EVENT, listener)
}

/** 显式拿某 device 的 transport。*/
export function getRemoteTransport(deviceId: number): Transport {
  return buildDeviceTransport(deviceId)
}

export function isDesktop(): boolean {
  return detectEnvironment() === "tauri"
}

/// True when the current window is a Tauri client bound to a remote
/// codeg-server (a remote-desktop window). Distinct from `isDesktop()`,
/// which is purely a runtime check — a remote-desktop window IS a Tauri
/// runtime but its API calls and file ops must target the remote host,
/// not the local filesystem.
export function isRemoteDesktopMode(): boolean {
  return _remoteTransport !== null
}

/// Base URL of the codeg server backing the current transport, for building
/// raw resource URLs that bypass the JSON transport (iframes, downloads).
/// Remote-desktop → the remote host; web → this page's origin. In pure-desktop
/// mode there's no server (callers use loopback / `invoke`), so this returns
/// the local origin only as a harmless fallback.
export function getServerBaseUrl(): string {
  if (_remoteConfig) return _remoteConfig.baseUrl.replace(/\/+$/, "")
  return typeof window !== "undefined" ? window.location.origin : ""
}

/// Surface a remote-server 401 to the same UI the transport uses for its
/// own auth failures. Direct `invoke()` calls (workspace file
/// upload/download) bypass `RemoteDesktopTransport.call`, so without
/// this they'd toast "token invalid" but never raise the
/// `connection-expired` dialog the rest of the app uses. Calling this on
/// a non-remote-desktop window is a no-op so the helper is safe to use
/// unconditionally from a 401 catch block.
export function notifyRemoteDesktopUnauthorized(): void {
  _remoteConfig?.onUnauthorized?.()
}

/**
 * Test-only: clear the cached shell + remote transports so a subsequent
 * `getTransport()` / `getShellTransport()` call re-runs environment detection
 * against the current `window` mock. The module-level singletons would
 * otherwise stick across test cases. Not intended for production use.
 * @internal
 */
export function __resetTransportForTests(): void {
  // Hard guard: collapses to a no-op outside vitest. Turbopack/webpack DCE
  // the dead branch in `next build` so the function ships as a single
  // `return` in the prod bundle instead of reaching into module state.
  if (process.env.NODE_ENV !== "test") return
  _shellTransport?.destroy?.()
  _remoteTransport?.destroy?.()
  _shellTransport = null
  _remoteTransport = null
  _remoteConfig = null
}
