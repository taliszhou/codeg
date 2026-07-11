"use client"

import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Loader2, Plus, Pencil, Trash2, Globe } from "lucide-react"
import {
  listDeviceServices,
  createDeviceService,
  updateDeviceService,
  deleteDeviceService,
  type DeviceServiceInfo,
  type RemoteDeviceMasked,
} from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

interface Props {
  device: RemoteDeviceMasked | null
  onOpenChange: (open: boolean) => void
}

export function DeviceServicesDialog({ device, onOpenChange }: Props) {
  const [services, setServices] = useState<DeviceServiceInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [editing, setEditing] = useState<DeviceServiceInfo | null>(null)
  const [draft, setDraft] = useState<{ name: string; url: string }>({ name: "", url: "" })

  const refresh = useCallback(async () => {
    if (!device) return
    setLoading(true)
    try {
      const list = await listDeviceServices(device.id)
      setServices(list)
    } catch (e) {
      toast.error("加载失败", { description: e instanceof Error ? e.message : String(e) })
    } finally {
      setLoading(false)
    }
  }, [device])

  useEffect(() => {
    if (device) {
      refresh()
      setEditing(null)
      setDraft({ name: "", url: "" })
    }
  }, [device, refresh])

  const handleSave = async () => {
    if (!device || !draft.name.trim() || !draft.url.trim()) {
      toast.error("name 和 url 不能为空")
      return
    }
    try {
      if (editing) {
        await updateDeviceService({
          id: editing.id,
          name: draft.name,
          url: draft.url,
        })
        toast.success("已更新")
      } else {
        await createDeviceService({
          deviceId: device.id,
          name: draft.name,
          url: draft.url,
        })
        toast.success("已添加")
      }
      setDraft({ name: "", url: "" })
      setEditing(null)
      await refresh()
    } catch (e) {
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) })
    }
  }

  const handleToggle = async (svc: DeviceServiceInfo) => {
    try {
      await updateDeviceService({ id: svc.id, enabled: !svc.enabled })
      setServices((prev) =>
        prev.map((s) => (s.id === svc.id ? { ...s, enabled: !s.enabled } : s))
      )
    } catch (e) {
      toast.error("切换失败", { description: e instanceof Error ? e.message : String(e) })
    }
  }

  const handleDelete = async (svc: DeviceServiceInfo) => {
    if (!confirm(`删除服务 "${svc.name}"?`)) return
    try {
      await deleteDeviceService(svc.id)
      toast.success("已删除")
      await refresh()
    } catch (e) {
      toast.error("删除失败", { description: e instanceof Error ? e.message : String(e) })
    }
  }

  const startEdit = (svc: DeviceServiceInfo) => {
    setEditing(svc)
    setDraft({ name: svc.name, url: svc.url })
  }

  const cancelEdit = () => {
    setEditing(null)
    setDraft({ name: "", url: "" })
  }

  if (!device) return null

  return (
    <Dialog open={!!device} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Globe className="h-4 w-4" /> {device.name} — 内网服务
          </DialogTitle>
          <DialogDescription className="text-xs">
            这台设备能访问的内网服务清单 (如 NAS、Grafana、内部 wiki)。在浏览器模式
            出口切到此设备时,可在地址栏旁的"服务"下拉里一键打开。禁用的服务不显示在
            下拉,但保留配置。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-3 py-2">
          {/* 添加/编辑表单 */}
          <div className="space-y-2 rounded border p-3 bg-muted/30">
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <Label className="text-xs">名称</Label>
                <Input
                  value={draft.name}
                  placeholder="家里 NAS"
                  onChange={(e) => setDraft((d) => ({ ...d, name: e.target.value }))}
                  className="h-8 text-sm"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">URL</Label>
                <Input
                  value={draft.url}
                  placeholder="http://192.168.1.10:5000"
                  onChange={(e) => setDraft((d) => ({ ...d, url: e.target.value }))}
                  className="h-8 text-sm font-mono"
                />
              </div>
            </div>
            <div className="flex gap-2 justify-end">
              {editing && (
                <Button size="sm" variant="ghost" onClick={cancelEdit}>
                  取消编辑
                </Button>
              )}
              <Button size="sm" onClick={handleSave}>
                <Plus className="h-3.5 w-3.5 mr-1" />
                {editing ? "保存修改" : "添加服务"}
              </Button>
            </div>
          </div>

          {/* 服务列表 */}
          {loading ? (
            <div className="flex items-center justify-center py-6">
              <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
            </div>
          ) : services.length === 0 ? (
            <div className="rounded border border-dashed py-6 text-center text-xs text-muted-foreground">
              还没有服务。在上面表单里添加第一个。
            </div>
          ) : (
            <div className="space-y-1.5 max-h-[40vh] overflow-y-auto">
              {services.map((svc) => (
                <div
                  key={svc.id}
                  className={
                    "flex items-center gap-2 px-2 py-1.5 rounded border " +
                    (svc.enabled ? "" : "opacity-50 bg-muted/30")
                  }
                >
                  <Switch
                    checked={svc.enabled}
                    onCheckedChange={() => handleToggle(svc)}
                    title={svc.enabled ? "禁用" : "启用"}
                  />
                  <div className="flex-1 min-w-0">
                    <div className="text-sm font-medium truncate">{svc.name}</div>
                    <div className="text-xs text-muted-foreground font-mono truncate">{svc.url}</div>
                  </div>
                  <Button size="icon" variant="ghost" className="h-7 w-7" onClick={() => startEdit(svc)}>
                    <Pencil className="h-3.5 w-3.5" />
                  </Button>
                  <Button
                    size="icon"
                    variant="ghost"
                    className="h-7 w-7 text-destructive"
                    onClick={() => handleDelete(svc)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
              ))}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            完成
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
