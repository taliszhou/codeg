import type { EventStream, Transport, UnsubscribeFn } from "./types"
import type { AttachTransportHost } from "./web-event-stream"
import { WebEventStream } from "./web-event-stream"

interface WebEvent {
  channel: string
  payload: unknown
}

function getToken(): string {
  return localStorage.getItem("codeg_token") ?? ""
}

/**
 * 远程设备 transport: 所有请求经本机 codeg-server `/api/remote/:id/api/*`
 * 再透传到远端。鉴权用本机 token; 远端 token 不出本机 server。
 *
 * 注: 该 transport 仍用本机 baseUrl + 本机 token,跟 WebTransport 区别只在
 * URL 前缀加 `/remote/:deviceId`。
 */
export class RemoteWebTransport implements Transport {
  private ws: WebSocket | null = null
  private handlers = new Map<string, Set<(payload: unknown) => void>>()
  private baseUrl: string
  private deviceId: number
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private wsFailCount = 0
  // attach 协议管线。EventStream 在首次 eventStream() 时惰性创建,存活整个
  // transport 生命周期,每次 WS-ready 时重发各订阅的 attach 帧。复用与本地
  // WebTransport 同一份 WebEventStream,差异只在 WS URL 前缀和 token 传法。
  private wsOpen = false
  private wsReadyCallbacks = new Set<() => void>()
  private eventStreamInstance: WebEventStream | null = null

  constructor(baseUrl: string, deviceId: number) {
    this.baseUrl = baseUrl
    this.deviceId = deviceId
  }

  async call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const token = getToken()
    const res = await fetch(
      `${this.baseUrl}/api/remote/${this.deviceId}/api/${command}`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify(args ?? {}),
      }
    )
    if (res.status === 401) {
      throw new Error("Unauthorized on remote device")
    }
    if (!res.ok) {
      const error = await res.json().catch(() => ({
        code: "network_error",
        message: `HTTP ${res.status}`,
      }))
      throw error
    }
    return res.json()
  }

  async subscribe<T>(
    event: string,
    handler: (payload: T) => void
  ): Promise<UnsubscribeFn> {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set())
    }
    const wrappedHandler = handler as (payload: unknown) => void
    this.handlers.get(event)!.add(wrappedHandler)

    if (!this.ws && getToken()) {
      this.connectWs()
    }

    return () => {
      this.handlers.get(event)?.delete(wrappedHandler)
    }
  }

  isDesktop(): boolean {
    return false
  }

  eventStream(): EventStream {
    if (!this.eventStreamInstance) {
      const host: AttachTransportHost = {
        isWsOpen: () => this.wsOpen,
        sendFrame: (frame) => this.sendWsFrame(frame),
        onWsReady: (callback) => {
          this.wsReadyCallbacks.add(callback)
          return () => {
            this.wsReadyCallbacks.delete(callback)
          }
        },
      }
      this.eventStreamInstance = new WebEventStream(host)
      // attach 帧需要一条已开的 WS 才能落地;若 legacy subscribe 还没把
      // WS 连起来,这里主动连一次。
      if (!this.ws && getToken()) {
        this.connectWs()
      }
    }
    return this.eventStreamInstance
  }

  private sendWsFrame(frame: object): boolean {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false
    try {
      this.ws.send(JSON.stringify(frame))
      return true
    } catch (err) {
      console.warn("[RemoteWebTransport] sendWsFrame failed:", err)
      return false
    }
  }

  private connectWs() {
    const token = getToken()
    if (!token) return

    const wsUrl =
      this.baseUrl.replace(/^http/, "ws") +
      `/api/remote/${this.deviceId}/ws/events?token=${encodeURIComponent(token)}`
    console.log("[RemoteWS] connecting", wsUrl.replace(/token=.*/, "token=***"))
    this.ws = new WebSocket(wsUrl)

    this.ws.onopen = () => {
      console.log("[RemoteWS] open, notifying", this.wsReadyCallbacks.size, "callbacks")
      this.wsFailCount = 0
      this.wsOpen = true
      // 通知 EventStream 在(重)连后按各订阅的 lastAppliedSeq 重发 attach 帧。
      for (const cb of this.wsReadyCallbacks) {
        try {
          cb()
        } catch (err) {
          console.error("[RemoteWebTransport] wsReady callback threw:", err)
        }
      }
    }

    this.ws.onmessage = (msg) => {
      try {
        const parsed = JSON.parse(msg.data) as unknown
        // attach 协议帧带 `type` 判别字段;legacy 全局广播帧带 `channel`。
        // 按哪个字段在场来路由,让两类帧共用同一条 WS。
        if (
          parsed &&
          typeof parsed === "object" &&
          "type" in (parsed as object)
        ) {
          this.eventStreamInstance?.handleServerFrame(parsed)
          return
        }
        const event = parsed as WebEvent
        const handlers = this.handlers.get(event.channel)
        if (handlers) {
          for (const h of handlers) {
            h(event.payload)
          }
        }
      } catch {
        // ignore malformed
      }
    }

    this.ws.onclose = () => {
      console.log("[RemoteWS] closed, failCount=", this.wsFailCount)
      this.ws = null
      this.wsOpen = false
      this.wsFailCount++
      if (this.wsFailCount >= 5) return
      this.reconnectTimer = setTimeout(() => this.connectWs(), 3000)
    }

    this.ws.onerror = () => {
      this.ws?.close()
    }
  }

  destroy() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
    }
    this.eventStreamInstance?.destroy()
    this.eventStreamInstance = null
    this.ws?.close()
    this.ws = null
    this.wsOpen = false
    this.handlers.clear()
  }
}
