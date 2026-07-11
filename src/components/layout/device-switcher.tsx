"use client"

import { useCallback, useEffect, useState } from "react"
import { Network, Smartphone, ChevronDown, RefreshCw } from "lucide-react"
import { useRouter } from "next/navigation"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  getActiveDeviceId,
  setActiveDeviceId,
  subscribeActiveDevice,
} from "@/lib/transport"
import { listRemoteDevices, type RemoteDeviceMasked } from "@/lib/api"

/**
 * Sidebar 顶部的设备切换器。
 * - 选"本机"时调用都走 local
 * - 选某远端设备时所有 workspace 内的 transport 调用透传到该设备
 * - settings 路由不受影响 (transport 内部判断 pathname)
 */
export function DeviceSwitcher() {
  const router = useRouter()
  const [devices, setDevices] = useState<RemoteDeviceMasked[]>([])
  const [activeId, setActiveIdState] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)

  const refresh = useCallback(async () => {
    try {
      const list = await listRemoteDevices()
      setDevices(list)
    } catch (e) {
      console.warn("[device-switcher] list failed", e)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
    setActiveIdState(getActiveDeviceId())
    const unsub = subscribeActiveDevice((id) => setActiveIdState(id))
    return unsub
  }, [refresh])

  const switchTo = useCallback(
    (deviceId: number | null) => {
      setActiveDeviceId(deviceId)
      // 强制 router refresh 让 server components 重 fetch (虽然 workspace 是 client 组件,
      // 也保险点)
      router.refresh()
    },
    [router]
  )

  const activeDevice = devices.find((d) => d.id === activeId)
  const label = activeId == null ? "本机" : activeDevice?.name ?? `设备 ${activeId}`

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          className="flex w-full items-center justify-between gap-2 rounded-md border border-border bg-background/50 px-2.5 py-1.5 text-xs font-medium hover:bg-accent transition-colors"
          title="切换设备"
        >
          <div className="flex items-center gap-1.5 min-w-0">
            {activeId == null ? (
              <Smartphone className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
            ) : (
              <Network className="h-3.5 w-3.5 text-emerald-600 shrink-0" />
            )}
            <span className="truncate">{label}</span>
          </div>
          <ChevronDown className="h-3 w-3 opacity-60 shrink-0" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        <DropdownMenuLabel className="text-xs">查看设备的工作区</DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => switchTo(null)}
          className={activeId == null ? "bg-accent" : ""}
        >
          <Smartphone className="h-3.5 w-3.5 mr-2" /> 本机
        </DropdownMenuItem>
        {devices.length > 0 && <DropdownMenuSeparator />}
        {loading && devices.length === 0 ? (
          <DropdownMenuItem disabled>
            <RefreshCw className="h-3.5 w-3.5 mr-2 animate-spin" /> 加载中
          </DropdownMenuItem>
        ) : (
          devices.map((d) => (
            <DropdownMenuItem
              key={d.id}
              onClick={() => switchTo(d.id)}
              className={activeId === d.id ? "bg-accent" : ""}
            >
              <Network className="h-3.5 w-3.5 mr-2 text-emerald-600" />
              <div className="flex flex-col min-w-0">
                <span className="truncate">{d.name}</span>
                <span className="text-[10px] text-muted-foreground truncate font-mono">
                  {d.base_url}
                </span>
              </div>
            </DropdownMenuItem>
          ))
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => router.push("/settings/remote-devices")}>
          + 添加远程设备
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
