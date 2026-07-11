"use client"

import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import {
  Globe,
  Loader2,
  Power,
  Copy,
  RefreshCw,
  ShieldAlert,
  CheckCircle2,
  XCircle,
  Smartphone,
  Network,
} from "lucide-react"
import {
  getForwardProxyStatus,
  updateForwardProxy,
  regenerateForwardProxyToken,
  listRemoteDevices,
  type ForwardProxyStatus,
  type RemoteDeviceMasked,
} from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

export default function ForwardProxyPage() {
  const [status, setStatus] = useState<ForwardProxyStatus | null>(null)
  const [devices, setDevices] = useState<RemoteDeviceMasked[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [portInput, setPortInput] = useState("8118")
  const [showToken, setShowToken] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const [s, ds] = await Promise.all([getForwardProxyStatus(), listRemoteDevices()])
      setStatus(s)
      setDevices(ds)
      setPortInput(String(s.listenPort))
    } catch (e) {
      toast.error("加载失败", {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  const handleToggle = async (enabled: boolean) => {
    if (!status) return
    const port = parseInt(portInput, 10)
    if (isNaN(port) || port < 1 || port > 65535) {
      toast.error("端口必须是 1–65535")
      return
    }
    setBusy(true)
    try {
      const next = await updateForwardProxy({ enabled, listenPort: port })
      setStatus(next)
      toast.success(enabled ? `已启用,监听 0.0.0.0:${port}` : "已停止")
    } catch (e) {
      toast.error("操作失败", {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setBusy(false)
    }
  }

  const handleUpstreamChange = async (value: string) => {
    if (!status) return
    const port = parseInt(portInput, 10)
    const upstreamDeviceId = value === "local" ? null : parseInt(value, 10)
    setBusy(true)
    try {
      const next = await updateForwardProxy({
        enabled: status.enabled,
        listenPort: isNaN(port) ? status.listenPort : port,
        upstreamDeviceId,
      })
      setStatus(next)
      toast.success(
        upstreamDeviceId == null
          ? "已切换到本机出口"
          : `已切换到设备 ${devices.find((d) => d.id === upstreamDeviceId)?.name ?? upstreamDeviceId} 出口`
      )
    } catch (e) {
      toast.error("切换出口失败", { description: e instanceof Error ? e.message : String(e) })
    } finally {
      setBusy(false)
    }
  }

  const handlePortBlur = async () => {
    if (!status || !status.enabled) return
    const port = parseInt(portInput, 10)
    if (isNaN(port) || port === status.listenPort) return
    if (port < 1 || port > 65535) {
      toast.error("端口必须是 1–65535")
      setPortInput(String(status.listenPort))
      return
    }
    setBusy(true)
    try {
      const next = await updateForwardProxy({ enabled: true, listenPort: port })
      setStatus(next)
      toast.success(`已切换到端口 ${port}`)
    } catch (e) {
      toast.error("切换端口失败", {
        description: e instanceof Error ? e.message : String(e),
      })
      setPortInput(String(status.listenPort))
    } finally {
      setBusy(false)
    }
  }

  const handleRegenerate = async () => {
    if (!confirm("重新生成 token 会让所有正在使用的客户端立刻断开,需要重新配置。继续?")) return
    setBusy(true)
    try {
      const next = await regenerateForwardProxyToken()
      setStatus(next)
      toast.success("新 token 已生成")
    } catch (e) {
      toast.error("重新生成失败", {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setBusy(false)
    }
  }

  const copyToken = () => {
    if (!status) return
    navigator.clipboard.writeText(status.token)
    toast.success("token 已复制到剪贴板")
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
    )
  }
  if (!status) return null

  const proxyExample = `http://anyuser:${status.token}@<this-machine-ip>:${status.runningPort ?? status.listenPort}`

  return (
    <div className="p-4 space-y-4">
      <div className="flex items-start gap-3">
        <Globe className="h-6 w-6 mt-0.5 text-emerald-600 shrink-0" />
        <div>
          <h1 className="text-xl font-semibold">网络跳板</h1>
          <p className="text-sm text-muted-foreground max-w-2xl">
            把 mobilega-server 当成 HTTP/HTTPS forward proxy。手机或其他设备配置代理后,
            所有流量经过此 server 出口,可访问 server 本机能访问的任何网络
            (家庭内网、VPN 后的资源等)。
          </p>
        </div>
      </div>

      <Card className="border-amber-400/60 bg-amber-50 dark:bg-amber-950/20">
        <CardContent className="flex items-start gap-2 py-3">
          <ShieldAlert className="h-4 w-4 mt-0.5 text-amber-600 shrink-0" />
          <p className="text-xs text-amber-900 dark:text-amber-300">
            <b>安全警告</b>: 启用后任何拿到 token 的人都能通过此 server 当出口节点访问外网。
            <b>不要在公共 WiFi 启用</b>。token 必须严格保密,泄漏后立刻重新生成。
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base flex items-center gap-2">
            <Power className="h-4 w-4" /> 启停 & 端口
          </CardTitle>
          <CardDescription className="flex items-center gap-2 text-xs">
            状态:
            {status.running ? (
              <span className="inline-flex items-center gap-1 text-emerald-600">
                <CheckCircle2 className="h-3.5 w-3.5" />
                运行中,监听 0.0.0.0:{status.runningPort}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1 text-muted-foreground">
                <XCircle className="h-3.5 w-3.5" />
                未运行
              </span>
            )}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-center justify-between gap-3 rounded-md border px-3 py-2">
            <div>
              <div className="text-sm font-medium">启用 forward proxy</div>
              <p className="text-xs text-muted-foreground">
                关闭后端口立即释放,正在用此 proxy 的客户端连接会断开。
              </p>
            </div>
            <Switch checked={status.enabled} onCheckedChange={handleToggle} disabled={busy} />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="fp-port">监听端口</Label>
            <Input
              id="fp-port"
              type="number"
              value={portInput}
              onChange={(e) => setPortInput(e.target.value)}
              onBlur={handlePortBlur}
              min={1}
              max={65535}
              disabled={busy}
              className="max-w-[160px]"
            />
            <p className="text-xs text-muted-foreground">
              默认 8118 (隐私敏感的常见 HTTP proxy 端口)。修改后离开输入框自动应用。
            </p>
          </div>

          <div className="space-y-1.5">
            <Label>出口设备 (Phase 4b 跳板)</Label>
            <Select
              value={status.upstreamDeviceId == null ? "local" : String(status.upstreamDeviceId)}
              onValueChange={handleUpstreamChange}
              disabled={busy}
            >
              <SelectTrigger className="max-w-sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="local">
                  <span className="inline-flex items-center gap-2">
                    <Smartphone className="h-3.5 w-3.5" /> 本机出口 (默认)
                  </span>
                </SelectItem>
                {devices.map((d) => (
                  <SelectItem key={d.id} value={String(d.id)}>
                    <span className="inline-flex items-center gap-2">
                      <Network className="h-3.5 w-3.5 text-emerald-600" /> {d.name}
                      <span className="text-[10px] text-muted-foreground font-mono">
                        ({d.base_url})
                      </span>
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              选远端设备后,所有客户端流量先经本机 forward proxy → 通过 C1 链路转到该
              设备 → 用该设备的网络出口访问 upstream。适合 "PC 在家、手机在外面" 场景。
            </p>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">鉴权 Token</CardTitle>
          <CardDescription className="text-xs">
            客户端必须在请求里带 <code className="bg-muted px-1 rounded">Proxy-Authorization: Bearer &lt;token&gt;</code>{" "}
            或 Basic auth (用户名任意,密码 = token)。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex items-center gap-2">
            <code className="flex-1 bg-muted px-2 py-1.5 rounded text-xs font-mono break-all">
              {showToken
                ? status.token
                : status.token.slice(0, 4) + "•".repeat(20) + status.token.slice(-4)}
            </code>
            <Button variant="outline" size="sm" onClick={() => setShowToken((v) => !v)}>
              {showToken ? "隐藏" : "显示"}
            </Button>
            <Button variant="outline" size="sm" onClick={copyToken}>
              <Copy className="h-3.5 w-3.5" />
            </Button>
          </div>
          <Button variant="outline" size="sm" onClick={handleRegenerate} disabled={busy}>
            <RefreshCw className="h-3.5 w-3.5 mr-1" /> 重新生成 token
          </Button>
        </CardContent>
      </Card>

      {status.running && (
        <Card className="border-emerald-400/60 bg-emerald-50 dark:bg-emerald-950/20">
          <CardHeader>
            <CardTitle className="text-base text-emerald-700 dark:text-emerald-300">
              客户端配置示例
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 text-xs">
            <div>
              <div className="font-semibold mb-1">浏览器/系统 proxy 设置 (HTTP &amp; HTTPS):</div>
              <code className="block bg-muted px-2 py-1.5 rounded font-mono break-all">
                {proxyExample}
              </code>
            </div>
            <div>
              <div className="font-semibold mb-1 mt-2">curl 测试:</div>
              <code className="block bg-muted px-2 py-1.5 rounded font-mono break-all whitespace-pre-wrap">
                curl -x http://&lt;this-machine-ip&gt;:{status.runningPort} \{"\n"}
                {"  "}-U anyuser:{showToken ? status.token : "<token>"} \{"\n"}
                {"  "}https://example.com/
              </code>
            </div>
            <p className="text-muted-foreground mt-2">
              <code>&lt;this-machine-ip&gt;</code> 替换成 LAN IP (mac 上 `ipconfig getifaddr en0`) 或公网 DDNS 域名。
            </p>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
