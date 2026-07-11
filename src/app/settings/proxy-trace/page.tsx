"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { ActivitySquare, Loader2, RefreshCw, Filter } from "lucide-react"
import { listProxyTraces, type ProxyTraceEntry } from "@/lib/api"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"

const LABELS = ["remote-device", "service-proxy", "forward-proxy", "browser-proxy"] as const

function fmtTime(ts: number): string {
  const d = new Date(ts)
  return (
    d.toLocaleTimeString([], { hour12: false }) +
    "." +
    String(d.getMilliseconds()).padStart(3, "0")
  )
}

function fmtBytes(n: number): string {
  if (!n) return "0"
  const units = ["B", "K", "M", "G"]
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(i >= 2 ? 1 : 0)}${units[i]}`
}

function statusClass(s: number | null): string {
  if (s == null) return "text-destructive"
  if (s >= 500) return "text-destructive"
  if (s >= 400) return "text-amber-600 dark:text-amber-400"
  if (s >= 300) return "text-blue-600 dark:text-blue-400"
  if (s >= 200) return "text-emerald-600 dark:text-emerald-400"
  return "text-muted-foreground"
}

function labelClass(l: string): string {
  switch (l) {
    case "remote-device":
      return "bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300"
    case "forward-proxy":
      return "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300"
    case "browser-proxy":
      return "bg-violet-100 text-violet-700 dark:bg-violet-900/40 dark:text-violet-300"
    case "service-proxy":
      return "bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300"
    default:
      return "bg-muted text-muted-foreground"
  }
}

export default function ProxyTracePage() {
  const [traces, setTraces] = useState<ProxyTraceEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [autoRefresh, setAutoRefresh] = useState(true)
  const [filter, setFilter] = useState<Set<string>>(new Set(LABELS))
  const [search, setSearch] = useState("")

  const refresh = useCallback(async () => {
    try {
      const list = await listProxyTraces(200)
      setTraces(list)
    } catch (e) {
      console.error("[proxy-trace] fetch", e)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
    if (!autoRefresh) return
    const tick = setInterval(refresh, 2000)
    return () => clearInterval(tick)
  }, [refresh, autoRefresh])

  const visible = useMemo(() => {
    return traces.filter((t) => {
      if (!filter.has(t.label)) return false
      if (search && !t.upstream.toLowerCase().includes(search.toLowerCase())) return false
      return true
    })
  }, [traces, filter, search])

  const toggleLabel = (l: string) => {
    setFilter((prev) => {
      const next = new Set(prev)
      if (next.has(l)) next.delete(l)
      else next.add(l)
      return next
    })
  }

  return (
    <div className="p-4 space-y-3 h-full flex flex-col">
      <div className="flex items-start gap-3 shrink-0">
        <ActivitySquare className="h-6 w-6 mt-0.5 text-violet-600 shrink-0" />
        <div className="flex-1">
          <h1 className="text-xl font-semibold">代理调试</h1>
          <p className="text-sm text-muted-foreground">
            实时显示最近 200 条代理流量 (C1 远程设备 / C3 forward proxy / C4 内置浏览器)。
            自动每 2 秒刷新。
          </p>
        </div>
      </div>

      <div className="flex items-center gap-2 shrink-0">
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="按 upstream 搜索 (eg. example.com)"
          className="flex-1 h-8 px-2 text-sm rounded border bg-background"
        />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="sm" className="h-8 text-xs gap-1">
              <Filter className="h-3.5 w-3.5" /> 类型 ({filter.size}/{LABELS.length})
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuLabel>过滤类型</DropdownMenuLabel>
            <DropdownMenuSeparator />
            {LABELS.map((l) => (
              <DropdownMenuCheckboxItem
                key={l}
                checked={filter.has(l)}
                onCheckedChange={() => toggleLabel(l)}
              >
                <span className={`px-1.5 py-0.5 rounded text-[10px] mr-2 ${labelClass(l)}`}>
                  {l}
                </span>
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant={autoRefresh ? "default" : "outline"}
          size="sm"
          className="h-8 text-xs gap-1"
          onClick={() => setAutoRefresh((v) => !v)}
        >
          <RefreshCw className={`h-3.5 w-3.5 ${autoRefresh ? "animate-spin" : ""}`} />
          {autoRefresh ? "自动" : "暂停"}
        </Button>
        <Button variant="ghost" size="sm" className="h-8 text-xs" onClick={refresh}>
          手动刷新
        </Button>
      </div>

      {loading ? (
        <div className="flex items-center justify-center py-12">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : visible.length === 0 ? (
        <div className="rounded-md border border-dashed py-12 text-center text-sm text-muted-foreground">
          没有匹配的代理流量记录
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-auto border rounded-md">
          <table className="w-full text-xs">
            <thead className="sticky top-0 bg-muted/80 backdrop-blur z-10">
              <tr className="text-left">
                <th className="px-2 py-1.5 font-medium w-[100px]">时间</th>
                <th className="px-2 py-1.5 font-medium w-[100px]">类型</th>
                <th className="px-2 py-1.5 font-medium w-[60px]">方法</th>
                <th className="px-2 py-1.5 font-medium">Upstream</th>
                <th className="px-2 py-1.5 font-medium w-[60px]">状态</th>
                <th className="px-2 py-1.5 font-medium w-[70px]">耗时</th>
                <th className="px-2 py-1.5 font-medium w-[80px]">↑/↓</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((t, idx) => (
                <tr
                  key={`${t.tsMs}-${idx}`}
                  className="border-t hover:bg-muted/30 font-mono"
                  title={t.error ?? undefined}
                >
                  <td className="px-2 py-1 text-muted-foreground">{fmtTime(t.tsMs)}</td>
                  <td className="px-2 py-1">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] ${labelClass(t.label)}`}>
                      {t.label}
                    </span>
                  </td>
                  <td className="px-2 py-1">{t.method}</td>
                  <td className="px-2 py-1 truncate max-w-[400px]" title={t.upstream}>
                    {t.upstream}
                  </td>
                  <td className={`px-2 py-1 ${statusClass(t.status)}`}>
                    {t.status ?? "ERR"}
                  </td>
                  <td className="px-2 py-1 text-muted-foreground">{t.elapsedMs}ms</td>
                  <td className="px-2 py-1 text-muted-foreground">
                    {fmtBytes(t.bytesIn)}/{fmtBytes(t.bytesOut)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
