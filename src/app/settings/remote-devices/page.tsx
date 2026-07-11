"use client"

import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import {
  Loader2,
  Plus,
  Pencil,
  Trash2,
  Network,
  ShieldCheck,
  ShieldAlert,
  Globe,
} from "lucide-react"
import { DeviceServicesDialog } from "@/components/settings/device-services-dialog"
import {
  listRemoteDevices,
  createRemoteDevice,
  updateRemoteDevice,
  deleteRemoteDevice,
  testRemoteDevice,
  type RemoteDeviceMasked,
} from "@/lib/api"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"

interface DialogState {
  open: boolean
  editing: RemoteDeviceMasked | null
  name: string
  baseUrl: string
  token: string
  testing: boolean
  testResult: { ok: boolean; message: string | null } | null
}

const blankDialog: DialogState = {
  open: false,
  editing: null,
  name: "",
  baseUrl: "",
  token: "",
  testing: false,
  testResult: null,
}

export default function RemoteDevicesPage() {
  const [devices, setDevices] = useState<RemoteDeviceMasked[]>([])
  const [loading, setLoading] = useState(true)
  const [dialog, setDialog] = useState<DialogState>(blankDialog)
  const [confirmDelete, setConfirmDelete] = useState<RemoteDeviceMasked | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [servicesDevice, setServicesDevice] = useState<RemoteDeviceMasked | null>(null)

  const refresh = useCallback(async () => {
    try {
      const list = await listRemoteDevices()
      setDevices(list)
    } catch (e) {
      console.error("[remote-devices] list failed", e)
      toast.error("加载设备列表失败", {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  const openAdd = () => setDialog({ ...blankDialog, open: true })

  const openEdit = (d: RemoteDeviceMasked) =>
    setDialog({
      ...blankDialog,
      open: true,
      editing: d,
      name: d.name,
      baseUrl: d.base_url,
      token: "",  // 编辑时不预填 token (后端只返回 masked), 用户必须重新输入
    })

  const handleTest = async () => {
    if (!dialog.baseUrl.trim() || !dialog.token.trim()) {
      toast.error("请填写 base URL 和 token")
      return
    }
    setDialog((d) => ({ ...d, testing: true, testResult: null }))
    try {
      const result = await testRemoteDevice({
        baseUrl: dialog.baseUrl.trim(),
        token: dialog.token.trim(),
      })
      setDialog((d) => ({
        ...d,
        testing: false,
        testResult: { ok: result.ok, message: result.message },
      }))
    } catch (e) {
      setDialog((d) => ({
        ...d,
        testing: false,
        testResult: {
          ok: false,
          message: e instanceof Error ? e.message : String(e),
        },
      }))
    }
  }

  const handleSubmit = async () => {
    if (!dialog.name.trim()) {
      toast.error("请填写设备名称")
      return
    }
    if (!dialog.baseUrl.trim()) {
      toast.error("请填写 base URL")
      return
    }
    if (!dialog.token.trim()) {
      toast.error("请填写 token (编辑时也需要重新输入)")
      return
    }
    setSubmitting(true)
    try {
      if (dialog.editing) {
        await updateRemoteDevice({
          id: dialog.editing.id,
          name: dialog.name.trim(),
          baseUrl: dialog.baseUrl.trim(),
          token: dialog.token.trim(),
        })
        toast.success("已更新")
      } else {
        await createRemoteDevice({
          name: dialog.name.trim(),
          baseUrl: dialog.baseUrl.trim(),
          token: dialog.token.trim(),
        })
        toast.success("已添加")
      }
      setDialog(blankDialog)
      await refresh()
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      const detail = (e as { detail?: string })?.detail
      toast.error("保存失败", {
        description: detail ? `${msg}\n${detail}` : msg,
      })
    } finally {
      setSubmitting(false)
    }
  }

  const handleDelete = async () => {
    if (!confirmDelete) return
    try {
      await deleteRemoteDevice(confirmDelete.id)
      toast.success(`已删除 ${confirmDelete.name}`)
      setConfirmDelete(null)
      await refresh()
    } catch (e) {
      toast.error("删除失败", {
        description: e instanceof Error ? e.message : String(e),
      })
    }
  }

  return (
    <div className="p-4 space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold flex items-center gap-2">
            <Network className="h-5 w-5" /> 远程设备
          </h1>
          <p className="text-sm text-muted-foreground max-w-xl">
            添加另一台运行 codeg-server 的设备后,sidebar 会以三层结构显示
            (设备 / 工作目录 / 会话),你能在远程设备上直接打开历史会话继续对话,
            或在远程设备创建新会话/项目。base URL 可以是局域网 IP 也可以是路由器
            DDNS 公网域名。
          </p>
        </div>
        <Button onClick={openAdd}>
          <Plus className="h-4 w-4 mr-1" /> 添加设备
        </Button>
      </div>

      {loading ? (
        <div className="flex items-center justify-center py-12">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : devices.length === 0 ? (
        <div className="rounded-md border border-dashed py-12 text-center text-sm text-muted-foreground">
          还没有远程设备。点击"添加设备"配置一台。
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
          {devices.map((d) => (
            <Card key={d.id}>
              <CardHeader className="pb-2">
                <CardTitle className="text-base flex items-center gap-2">
                  <Network className="h-4 w-4 text-emerald-600" /> {d.name}
                </CardTitle>
                <CardDescription className="font-mono text-xs break-all">
                  {d.base_url}
                </CardDescription>
              </CardHeader>
              <CardContent className="text-xs text-muted-foreground space-y-1">
                <div>
                  <span className="opacity-70">token:</span>{" "}
                  <code className="bg-muted px-1.5 rounded">{d.token_masked}</code>
                </div>
                <div className="opacity-60">
                  添加于 {new Date(d.created_at).toLocaleString()}
                </div>
              </CardContent>
              <CardFooter className="gap-2 pt-0 flex-wrap">
                <Button variant="outline" size="sm" onClick={() => setServicesDevice(d)}>
                  <Globe className="h-3.5 w-3.5 mr-1" /> 管理服务
                </Button>
                <Button variant="outline" size="sm" onClick={() => openEdit(d)}>
                  <Pencil className="h-3.5 w-3.5 mr-1" /> 编辑
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-destructive"
                  onClick={() => setConfirmDelete(d)}
                >
                  <Trash2 className="h-3.5 w-3.5 mr-1" /> 删除
                </Button>
              </CardFooter>
            </Card>
          ))}
        </div>
      )}

      <Dialog
        open={dialog.open}
        onOpenChange={(open) => {
          if (!open) setDialog(blankDialog)
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {dialog.editing ? "编辑远程设备" : "添加远程设备"}
            </DialogTitle>
            <DialogDescription>
              指向运行 codeg-server 的设备。base URL 可以是 LAN IP、DDNS 域名,
              或带端口转发的公网地址。Token 是目标 server 启动时生成的 `CODEG_TOKEN`。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-2">
            <div className="space-y-1.5">
              <Label htmlFor="rd-name">名称</Label>
              <Input
                id="rd-name"
                placeholder="家里 PC"
                value={dialog.name}
                onChange={(e) => setDialog((d) => ({ ...d, name: e.target.value }))}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="rd-url">Base URL</Label>
              <Input
                id="rd-url"
                placeholder="https://home.example.com:8080"
                value={dialog.baseUrl}
                onChange={(e) => setDialog((d) => ({ ...d, baseUrl: e.target.value }))}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="rd-token">Token</Label>
              <Input
                id="rd-token"
                placeholder="远端 server 的 CODEG_TOKEN"
                value={dialog.token}
                type="password"
                onChange={(e) => setDialog((d) => ({ ...d, token: e.target.value }))}
              />
              <p className="text-xs text-muted-foreground">
                Token 仅明文保存在本机 SQLite,不会同步到任何云端。
              </p>
            </div>
            {dialog.testResult && (
              <div
                className={
                  dialog.testResult.ok
                    ? "rounded border border-emerald-400 bg-emerald-50 dark:bg-emerald-950/30 p-2 flex items-center gap-2 text-xs text-emerald-700 dark:text-emerald-300"
                    : "rounded border border-destructive bg-destructive/5 p-2 flex items-center gap-2 text-xs text-destructive"
                }
              >
                {dialog.testResult.ok ? (
                  <ShieldCheck className="h-4 w-4" />
                ) : (
                  <ShieldAlert className="h-4 w-4" />
                )}
                <span>
                  {dialog.testResult.ok
                    ? "连接成功 + token 有效"
                    : `连接失败: ${dialog.testResult.message ?? "unknown"}`}
                </span>
              </div>
            )}
          </div>
          <DialogFooter className="gap-2">
            <Button variant="outline" onClick={handleTest} disabled={dialog.testing}>
              {dialog.testing ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 mr-1 animate-spin" /> 测试中
                </>
              ) : (
                "测试连接"
              )}
            </Button>
            <Button variant="ghost" onClick={() => setDialog(blankDialog)}>
              取消
            </Button>
            <Button onClick={handleSubmit} disabled={submitting}>
              {submitting ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 mr-1 animate-spin" /> 保存中
                </>
              ) : dialog.editing ? (
                "保存修改"
              ) : (
                "添加"
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <DeviceServicesDialog
        device={servicesDevice}
        onOpenChange={(o) => !o && setServicesDevice(null)}
      />

      <AlertDialog
        open={!!confirmDelete}
        onOpenChange={(open) => !open && setConfirmDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>删除设备?</AlertDialogTitle>
            <AlertDialogDescription>
              删除 &quot;{confirmDelete?.name}&quot; 后,sidebar 不会再显示这台设备,
              但它的远端 server 和数据保持不变。你可以随时再添加回来。
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete}>删除</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
