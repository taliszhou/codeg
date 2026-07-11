"use client"

import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import {
  listLocalNpuModels,
  downloadLocalNpuModel,
  getLocalNpuDownloadStatus,
  deleteLocalNpuModel,
  loadLocalNpuModel,
  unloadLocalNpuModel,
  chatTestLocalNpu,
  type LocalNpuModelCatalog,
  type LocalNpuDownloadTaskState,
  type LocalNpuChatTestResult,
} from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { Loader2, CheckCircle2, Download, Trash2, Power, MessageSquare, Sparkles } from "lucide-react"

export default function LocalModelsPage() {
  const [data, setData] = useState<LocalNpuModelCatalog | null>(null)
  const [busy, setBusy] = useState<Record<string, string | null>>({})
  const [downloads, setDownloads] = useState<Record<string, LocalNpuDownloadTaskState>>({})
  const [unloading, setUnloading] = useState(false)
  const [selfTest, setSelfTest] = useState<Record<string, LocalNpuChatTestResult | { error: string } | null>>({})
  const [retesting, setRetesting] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      const d = await listLocalNpuModels()
      setData(d)
    } catch (e) {
      console.error("[local-models] list failed", e)
    }
  }, [])

  useEffect(() => {
    refresh()
    const tick = setInterval(refresh, 5000)
    return () => clearInterval(tick)
  }, [refresh])

  const setModelBusy = (id: string, action: string | null) =>
    setBusy((prev) => ({ ...prev, [id]: action }))

  const handleDownload = async (id: string) => {
    setModelBusy(id, "downloading")
    try {
      const { task_id } = await downloadLocalNpuModel(id)
      setDownloads((prev) => ({
        ...prev,
        [id]: {
          task_id,
          model_id: id,
          total_bytes: 0,
          downloaded_bytes: 0,
          status: "downloading",
          error: null,
        },
      }))
      pollDownload(id, task_id)
      toast.success(`开始下载 ${id}`)
    } catch (e) {
      toast.error(`下载启动失败`, {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setModelBusy(id, null)
    }
  }

  const pollDownload = (modelId: string, taskId: string) => {
    const handle = setInterval(async () => {
      try {
        const state = await getLocalNpuDownloadStatus(taskId)
        setDownloads((prev) => ({ ...prev, [modelId]: state }))
        if (state.status === "completed") {
          clearInterval(handle)
          toast.success(`${modelId} 下载完成`)
          refresh()
        } else if (state.status === "error") {
          clearInterval(handle)
          toast.error(`${modelId} 下载失败`, { description: state.error ?? "unknown" })
          refresh()
        } else if (state.status === "cancelled") {
          clearInterval(handle)
          refresh()
        }
      } catch (e) {
        clearInterval(handle)
        console.error(e)
      }
    }, 2000)
  }

  const runSelfTest = async (id: string, opts?: { reuseToastId?: string | number }) => {
    const tid = opts?.reuseToastId ?? toast.loading(`正在向 ${id} 发送测试 prompt...`)
    setRetesting(id)
    try {
      const result = await chatTestLocalNpu(id)
      setSelfTest((prev) => ({ ...prev, [id]: result }))
      const replyPreview = result.reply.slice(0, 80) + (result.reply.length > 80 ? "…" : "")
      toast.success(`${id} self-test 成功 (${result.elapsedMs}ms)`, {
        id: tid,
        description: replyPreview || "(空回复)",
        duration: 8000,
      })
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setSelfTest((prev) => ({ ...prev, [id]: { error: msg } }))
      toast.error(`${id} self-test 失败`, { id: tid, description: msg, duration: 10000 })
    } finally {
      setRetesting(null)
    }
  }

  const handleActivate = async (id: string) => {
    setModelBusy(id, "activating")
    const tid = toast.loading(`正在激活 ${id} (模型 ~3-15s 加载到 GPU)...`)
    try {
      await loadLocalNpuModel(id)
      await refresh()
      // 激活成功 → 复用同一 toast id 进入"测试中"→"成功/失败"链
      toast.loading(`${id} 已加载, 测试推理中...`, { id: tid })
      await runSelfTest(id, { reuseToastId: tid })
    } catch (e) {
      toast.error(`激活失败`, {
        id: tid,
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setModelBusy(id, null)
    }
  }

  const handleUnload = async () => {
    setUnloading(true)
    const tid = toast.loading("正在卸载模型...")
    try {
      await unloadLocalNpuModel()
      await refresh()
      toast.success("已卸载", { id: tid })
    } catch (e) {
      toast.error("卸载失败", {
        id: tid,
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setUnloading(false)
    }
  }

  const handleDelete = async (id: string) => {
    if (!confirm(`确定删除模型 ${id}？文件不可恢复。`)) return
    setModelBusy(id, "deleting")
    try {
      await deleteLocalNpuModel(id)
      await refresh()
      toast.success(`已删除 ${id}`)
    } catch (e) {
      toast.error(`删除失败`, {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setModelBusy(id, null)
    }
  }

  const activeId = data?.active_model_id ?? null

  return (
    <div className="p-4 space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold">本地 NPU 模型市场</h1>
          <p className="text-sm text-muted-foreground">
            下载 Gemma 4 等模型到本机 NPU 加速推理。
            {activeId ? (
              <>
                {" "}当前激活: <span className="text-primary font-medium">{activeId}</span>。
              </>
            ) : (
              <> 当前未激活任何模型 — 点下方"激活"按钮选一个。</>
            )}
          </p>
        </div>
        {activeId && (
          <Button variant="outline" onClick={handleUnload} disabled={unloading}>
            {unloading ? (
              <><Loader2 className="h-4 w-4 animate-spin" /> 卸载中</>
            ) : (
              <><Power className="h-4 w-4" /> 卸载 ({activeId})</>
            )}
          </Button>
        )}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {(data?.models ?? []).map((m) => {
          const dl = downloads[m.id]
          const downloading = dl?.status === "downloading"
          const isActive = m.id === activeId
          const action = busy[m.id]
          const progress = dl && dl.total_bytes > 0
            ? Math.round((dl.downloaded_bytes / dl.total_bytes) * 100)
            : 0
          return (
            <Card key={m.id} className={isActive ? "border-primary border-2 ring-2 ring-primary/20" : undefined}>
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  {m.display_name}
                  {isActive && (
                    <span className="inline-flex items-center gap-1 text-xs rounded bg-primary text-primary-foreground px-2 py-0.5">
                      <CheckCircle2 className="h-3 w-3" /> 已激活
                    </span>
                  )}
                </CardTitle>
                <CardDescription>
                  {formatBytes(m.size_bytes)} · 预期 ~{m.estimated_tok_per_sec} tok/s
                </CardDescription>
                <CardDescription className="pt-1">{m.description}</CardDescription>
              </CardHeader>
              <CardContent className="text-xs text-muted-foreground break-all">
                {m.file_path ? `📦 ${m.file_path}` : `↗ ${m.url}`}
                {downloading && (
                  <div className="pt-2 space-y-1">
                    <Progress value={progress} className="h-2" />
                    <div className="flex justify-between text-xs">
                      <span>{formatBytes(dl.downloaded_bytes)} / {formatBytes(dl.total_bytes)}</span>
                      <span>{progress}%</span>
                    </div>
                  </div>
                )}
                {dl?.status === "error" && (
                  <div className="pt-2 text-destructive text-xs">下载失败: {dl.error ?? "unknown"}</div>
                )}
                {isActive && selfTest[m.id] && (() => {
                  const r = selfTest[m.id]!
                  if ("error" in r) {
                    return (
                      <div className="mt-3 p-2 rounded border border-destructive/50 bg-destructive/5">
                        <div className="text-xs font-medium text-destructive flex items-center gap-1">
                          <MessageSquare className="h-3 w-3" /> 自测失败
                        </div>
                        <div className="mt-1 text-xs text-destructive break-words">{r.error}</div>
                      </div>
                    )
                  }
                  return (
                    <div className="mt-3 p-2 rounded border border-emerald-500/40 bg-emerald-500/5">
                      <div className="text-xs font-medium text-emerald-700 dark:text-emerald-400 flex items-center gap-1">
                        <Sparkles className="h-3 w-3" /> 模型已回答 ({r.elapsedMs} ms, 服务={r.servedModel})
                      </div>
                      <div className="mt-1 text-xs text-muted-foreground italic">→ {r.prompt}</div>
                      <div className="mt-1 text-sm text-foreground whitespace-pre-wrap break-words">{r.reply || "(空回复)"}</div>
                    </div>
                  )
                })()}
              </CardContent>
              <CardFooter className="gap-2 flex-wrap">
                {!m.downloaded && !downloading && (
                  <Button onClick={() => handleDownload(m.id)} disabled={!!action}>
                    {action === "downloading" ? (
                      <><Loader2 className="h-4 w-4 animate-spin" /> 启动中</>
                    ) : (
                      <><Download className="h-4 w-4" /> 下载</>
                    )}
                  </Button>
                )}
                {m.downloaded && !isActive && (
                  <Button onClick={() => handleActivate(m.id)} disabled={!!action}>
                    {action === "activating" ? (
                      <><Loader2 className="h-4 w-4 animate-spin" /> 激活中</>
                    ) : (
                      <><Power className="h-4 w-4" /> 激活</>
                    )}
                  </Button>
                )}
                {isActive && (
                  <Button
                    variant="outline"
                    onClick={() => runSelfTest(m.id)}
                    disabled={retesting === m.id}
                  >
                    {retesting === m.id ? (
                      <><Loader2 className="h-4 w-4 animate-spin" /> 测试中</>
                    ) : (
                      <><MessageSquare className="h-4 w-4" /> 再测一次</>
                    )}
                  </Button>
                )}
                {m.downloaded && (
                  <Button variant="destructive" onClick={() => handleDelete(m.id)} disabled={!!action}>
                    {action === "deleting" ? (
                      <><Loader2 className="h-4 w-4 animate-spin" /> 删除中</>
                    ) : (
                      <><Trash2 className="h-4 w-4" /> 删除</>
                    )}
                  </Button>
                )}
              </CardFooter>
            </Card>
          )
        })}
      </div>

      <p className="text-xs text-muted-foreground pt-4">
        ⚠️ 实验性 - NPU 加速。模型加载耗时 5-22s(取决于大小);同一时间只能 active 一个模型。
        激活后 codeg-server 自动用作 LLM provider。
      </p>
    </div>
  )
}

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B"
  const units = ["B", "KB", "MB", "GB", "TB"]
  let value = bytes
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex++
  }
  return `${value.toFixed(unitIndex >= 2 ? 1 : 0)} ${units[unitIndex]}`
}
