"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  CheckCircle2,
  Loader2,
  Network,
  Pencil,
  Plus,
  Trash2,
  XCircle,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
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
import {
  listConnections,
  createConnection,
  updateConnection,
  deleteConnection,
  listSshConfigAliases,
  testConnection,
} from "@/lib/api"
import { subscribe } from "@/lib/platform"
import type {
  ConnectionConfig,
  ConnectionInput,
  ConnectionTestProgressEvent,
  ConnectionTestStage,
  ConnectionTestStageResult,
  SshAuthMethod,
  SshConfigEntry,
} from "@/lib/types"

const TEST_PROGRESS_EVENT = "connection://test_progress"

const STAGE_ORDER: ConnectionTestStage[] = [
  "dns_resolve",
  "tcp_connect",
  "ssh_auth",
  "remote_shell",
  "daemon_path_writable",
  "daemon_probe",
]

export function SshConnectionSettings() {
  const t = useTranslations("SshConnectionSettings")
  const [connections, setConnections] = useState<ConnectionConfig[]>([])
  const [loading, setLoading] = useState(true)
  const [formTarget, setFormTarget] = useState<
    { mode: "create" } | { mode: "edit"; connection: ConnectionConfig } | null
  >(null)
  const [deleteTarget, setDeleteTarget] = useState<ConnectionConfig | null>(
    null
  )
  const [testTarget, setTestTarget] = useState<ConnectionConfig | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    try {
      const list = await listConnections()
      setConnections(list)
    } catch (e) {
      console.error(e)
      toast.error(t("loadFailed"))
    } finally {
      setLoading(false)
    }
  }, [t])

  useEffect(() => {
    reload()
  }, [reload])

  const handleDelete = useCallback(async () => {
    if (!deleteTarget) return
    try {
      await deleteConnection(deleteTarget.id)
      toast.success(t("deleteSuccess"))
      setDeleteTarget(null)
      reload()
    } catch (e) {
      console.error(e)
      toast.error(t("deleteFailed"))
    }
  }, [deleteTarget, reload, t])

  return (
    <ScrollArea className="h-full">
      <Tabs defaultValue="list" className="w-full space-y-4 p-3 md:p-4">
        <section className="space-y-3">
          <div>
            <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
            <p className="text-sm text-muted-foreground">
              {t("sectionDescription")}
            </p>
          </div>
          <TabsList>
            <TabsTrigger value="list">{t("tabs.list")}</TabsTrigger>
            <TabsTrigger value="ssh_config">{t("tabs.sshConfig")}</TabsTrigger>
          </TabsList>
        </section>

        <TabsContent value="list" className="mt-0 space-y-3">
          <div className="flex items-center justify-end">
            <Button size="sm" onClick={() => setFormTarget({ mode: "create" })}>
              <Plus className="mr-1 h-4 w-4" />
              {t("addConnection")}
            </Button>
          </div>

          {loading ? (
            <div className="flex items-center justify-center py-8 text-muted-foreground">
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              {t("loading")}
            </div>
          ) : connections.length === 0 ? (
            <div className="rounded-md border border-dashed p-8 text-center text-sm text-muted-foreground">
              {t("emptyHint")}
            </div>
          ) : (
            <ul className="space-y-2">
              {connections.map((c) => (
                <ConnectionRow
                  key={c.id}
                  connection={c}
                  onEdit={() => setFormTarget({ mode: "edit", connection: c })}
                  onDelete={() => setDeleteTarget(c)}
                  onTest={() => setTestTarget(c)}
                />
              ))}
            </ul>
          )}
        </TabsContent>

        <TabsContent value="ssh_config" className="mt-0">
          <SshConfigImportTab onImported={reload} />
        </TabsContent>
      </Tabs>

      {formTarget && (
        <ConnectionFormDialog
          mode={formTarget.mode}
          initial={
            formTarget.mode === "edit" ? formTarget.connection : undefined
          }
          onClose={() => setFormTarget(null)}
          onSaved={() => {
            setFormTarget(null)
            reload()
          }}
        />
      )}

      {deleteTarget && (
        <AlertDialog
          open={true}
          onOpenChange={(open) => {
            if (!open) setDeleteTarget(null)
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("delete.title")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("delete.description", { name: deleteTarget.name })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <div className="rounded-md bg-muted p-3 text-xs text-muted-foreground">
              {t("delete.willKeep")}
              <ul className="mt-1 list-disc pl-5">
                <li>{t("delete.remoteFiles")}</li>
              </ul>
            </div>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("delete.cancel")}</AlertDialogCancel>
              <AlertDialogAction onClick={handleDelete}>
                {t("delete.confirm")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}

      {testTarget && (
        <TestConnectionDialog
          connection={testTarget}
          onClose={() => setTestTarget(null)}
        />
      )}
    </ScrollArea>
  )
}

// ── Row ──────────────────────────────────────────────────────────────────

function ConnectionRow({
  connection,
  onEdit,
  onDelete,
  onTest,
}: {
  connection: ConnectionConfig
  onEdit: () => void
  onDelete: () => void
  onTest: () => void
}) {
  const t = useTranslations("SshConnectionSettings")
  const target = connection.ssh_alias
    ? connection.ssh_alias
    : `${connection.ssh_user ?? ""}${
        connection.ssh_user ? "@" : ""
      }${connection.ssh_host ?? ""}${
        connection.ssh_port && connection.ssh_port !== 22
          ? `:${connection.ssh_port}`
          : ""
      }`

  return (
    <li className="flex items-center justify-between gap-3 rounded-md border p-3">
      <div className="flex min-w-0 flex-1 items-center gap-3">
        <Network className="h-5 w-5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate font-medium">{connection.name}</span>
            <Badge variant="secondary" className="shrink-0 text-xs">
              {connection.ssh_auth_method}
            </Badge>
            {connection.auto_connect && (
              <Badge variant="outline" className="shrink-0 text-xs">
                {t("badges.autoConnect")}
              </Badge>
            )}
          </div>
          <div className="truncate text-xs text-muted-foreground">
            {target || t("noTarget")}
          </div>
          {connection.last_connected_at && (
            <div className="text-xs text-muted-foreground">
              {t("lastConnected", {
                time: new Date(connection.last_connected_at).toLocaleString(),
              })}
            </div>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <Button size="sm" variant="ghost" onClick={onTest}>
          {t("testButton")}
        </Button>
        <Button size="icon" variant="ghost" onClick={onEdit}>
          <Pencil className="h-4 w-4" />
        </Button>
        <Button size="icon" variant="ghost" onClick={onDelete}>
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>
    </li>
  )
}

// ── Form dialog ──────────────────────────────────────────────────────────

function ConnectionFormDialog({
  mode,
  initial,
  onClose,
  onSaved,
}: {
  mode: "create" | "edit"
  initial?: ConnectionConfig
  onClose: () => void
  onSaved: () => void
}) {
  const t = useTranslations("SshConnectionSettings")
  const [name, setName] = useState(initial?.name ?? "")
  const [authMethod, setAuthMethod] = useState<SshAuthMethod>(
    initial?.ssh_auth_method ?? "key"
  )
  const [host, setHost] = useState(initial?.ssh_host ?? "")
  const [port, setPort] = useState<string>(
    initial?.ssh_port != null ? String(initial.ssh_port) : "22"
  )
  const [user, setUser] = useState(initial?.ssh_user ?? "")
  const [keyPath, setKeyPath] = useState(initial?.ssh_key_path ?? "")
  const [keyPassphrase, setKeyPassphrase] = useState("")
  const [password, setPassword] = useState("")
  const [proxyJump, setProxyJump] = useState(initial?.proxy_jump ?? "")
  const [daemonPath, setDaemonPath] = useState(
    initial?.daemon_path ?? "~/.codeg-remote"
  )
  const [autoConnect, setAutoConnect] = useState(initial?.auto_connect ?? false)
  const [advanced, setAdvanced] = useState(false)
  const [saving, setSaving] = useState(false)

  const buildInput = useCallback((): ConnectionInput => {
    const portNum = port.trim() === "" ? null : Number(port)
    return {
      name: name.trim(),
      kind: "ssh",
      ssh_host: host.trim() || null,
      ssh_user: user.trim() || null,
      ssh_port: portNum != null && Number.isFinite(portNum) ? portNum : null,
      ssh_alias: initial?.ssh_alias ?? null,
      ssh_key_path:
        authMethod === "key" && keyPath.trim() ? keyPath.trim() : null,
      ssh_auth_method: authMethod,
      proxy_jump: proxyJump.trim() || null,
      daemon_path: daemonPath.trim() || null,
      auto_connect: autoConnect,
    }
  }, [
    name,
    host,
    user,
    port,
    keyPath,
    authMethod,
    proxyJump,
    daemonPath,
    autoConnect,
    initial,
  ])

  const handleSave = useCallback(async () => {
    setSaving(true)
    try {
      const input = buildInput()
      if (!input.name) {
        toast.error(t("form.errNameRequired"))
        return
      }
      if (!input.ssh_host && !input.ssh_alias) {
        toast.error(t("form.errHostRequired"))
        return
      }
      if (mode === "create") {
        await createConnection({
          input,
          keyPassphrase: keyPassphrase || null,
          password: password || null,
        })
      } else if (initial) {
        await updateConnection({
          id: initial.id,
          input,
          keyPassphrase: keyPassphrase || null,
          password: password || null,
        })
      }
      toast.success(t("form.saveSuccess"))
      onSaved()
    } catch (e) {
      console.error(e)
      toast.error(t("form.saveFailed"))
    } finally {
      setSaving(false)
    }
  }, [buildInput, mode, initial, keyPassphrase, password, onSaved, t])

  return (
    <Dialog open={true} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[85vh] max-w-md overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            {mode === "create" ? t("form.titleCreate") : t("form.titleEdit")}
          </DialogTitle>
          <DialogDescription>{t("form.description")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="conn-name">{t("form.name")}</Label>
            <Input
              id="conn-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("form.namePlaceholder")}
            />
          </div>
          <div className="grid grid-cols-3 gap-2">
            <div className="col-span-2 space-y-1.5">
              <Label htmlFor="conn-host">{t("form.host")}</Label>
              <Input
                id="conn-host"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="host.example.com"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="conn-port">{t("form.port")}</Label>
              <Input
                id="conn-port"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                placeholder="22"
              />
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="conn-user">{t("form.user")}</Label>
            <Input
              id="conn-user"
              value={user}
              onChange={(e) => setUser(e.target.value)}
              placeholder="alice"
            />
          </div>
          <div className="space-y-1.5">
            <Label>{t("form.authMethod")}</Label>
            <Select
              value={authMethod}
              onValueChange={(v) => setAuthMethod(v as SshAuthMethod)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="key">{t("form.authKey")}</SelectItem>
                <SelectItem value="password">
                  {t("form.authPassword")}
                </SelectItem>
                <SelectItem value="ssh_config">
                  {t("form.authSshConfig")}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          {authMethod === "key" && (
            <>
              <div className="space-y-1.5">
                <Label htmlFor="conn-key">{t("form.privateKey")}</Label>
                <Input
                  id="conn-key"
                  value={keyPath}
                  onChange={(e) => setKeyPath(e.target.value)}
                  placeholder="~/.ssh/id_ed25519"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="conn-passphrase">{t("form.passphrase")}</Label>
                <Input
                  id="conn-passphrase"
                  type="password"
                  value={keyPassphrase}
                  onChange={(e) => setKeyPassphrase(e.target.value)}
                  placeholder={
                    initial && mode === "edit"
                      ? t("form.passphraseUnchanged")
                      : ""
                  }
                />
                <p className="text-xs text-muted-foreground">
                  {t("form.passphraseHelp")}
                </p>
              </div>
            </>
          )}
          {authMethod === "password" && (
            <div className="space-y-1.5">
              <Label htmlFor="conn-password">{t("form.password")}</Label>
              <Input
                id="conn-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder={
                  initial && mode === "edit" ? t("form.passwordUnchanged") : ""
                }
              />
            </div>
          )}

          <button
            type="button"
            className="w-full text-left text-xs font-medium text-muted-foreground hover:text-foreground"
            onClick={() => setAdvanced((v) => !v)}
          >
            {advanced ? "▾" : "▸"} {t("form.advanced")}
          </button>
          {advanced && (
            <div className="space-y-3 rounded-md border p-3">
              <div className="space-y-1.5">
                <Label htmlFor="conn-jump">{t("form.proxyJump")}</Label>
                <Input
                  id="conn-jump"
                  value={proxyJump}
                  onChange={(e) => setProxyJump(e.target.value)}
                  placeholder="bastion.example.com"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="conn-daemon">{t("form.daemonPath")}</Label>
                <Input
                  id="conn-daemon"
                  value={daemonPath}
                  onChange={(e) => setDaemonPath(e.target.value)}
                  placeholder="~/.codeg-remote"
                />
              </div>
              <div className="flex items-center justify-between">
                <Label htmlFor="conn-auto" className="cursor-pointer">
                  {t("form.autoConnect")}
                </Label>
                <Switch
                  id="conn-auto"
                  checked={autoConnect}
                  onCheckedChange={setAutoConnect}
                />
              </div>
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose} disabled={saving}>
            {t("form.cancel")}
          </Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving && <Loader2 className="mr-1 h-4 w-4 animate-spin" />}
            {t("form.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// ── Test connection dialog ───────────────────────────────────────────────

function TestConnectionDialog({
  connection,
  onClose,
}: {
  connection: ConnectionConfig
  onClose: () => void
}) {
  const t = useTranslations("SshConnectionSettings")
  const [progress, setProgress] = useState<
    Record<ConnectionTestStage, ConnectionTestStageResult | undefined>
  >({} as Record<ConnectionTestStage, ConnectionTestStageResult | undefined>)
  const [running, setRunning] = useState(true)
  // Generate the test_id inside the mount effect so the purity lint is happy.
  const testIdRef = useRef<string | null>(null)

  useEffect(() => {
    if (testIdRef.current === null) {
      testIdRef.current = `t_${Date.now()}_${Math.random()
        .toString(36)
        .slice(2, 8)}`
    }
    let cancelled = false
    let unsub: (() => void) | undefined
    const myTestId = testIdRef.current

    subscribe<ConnectionTestProgressEvent>(TEST_PROGRESS_EVENT, (payload) => {
      if (payload.test_id !== myTestId) return
      setProgress((prev) => ({
        ...prev,
        [payload.stage]: {
          stage: payload.stage,
          status: payload.status,
          elapsed_ms: payload.elapsed_ms,
          message: payload.message,
        },
      }))
    }).then((fn) => {
      if (cancelled) fn()
      else unsub = fn
    })

    testConnection({ id: connection.id, testId: myTestId })
      .catch((e) => {
        console.error(e)
        toast.error(t("test.failed"))
      })
      .finally(() => {
        if (!cancelled) setRunning(false)
      })

    return () => {
      cancelled = true
      unsub?.()
    }
  }, [connection.id, t])

  const allSuccess = useMemo(
    () =>
      STAGE_ORDER.every((s) => {
        const r = progress[s]
        return r && (r.status === "success" || r.status === "skipped")
      }) && !running,
    [progress, running]
  )

  return (
    <Dialog open={true} onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {t("test.title", { name: connection.name })}
          </DialogTitle>
        </DialogHeader>
        <ul className="space-y-2">
          {STAGE_ORDER.map((stage) => {
            const r = progress[stage]
            return (
              <li
                key={stage}
                className="flex items-start gap-3 rounded-md border p-2"
              >
                <StageIcon status={r?.status ?? "pending"} />
                <div className="flex-1">
                  <div className="flex items-center justify-between">
                    <span className="text-sm font-medium">
                      {t(`test.stages.${stage}`)}
                    </span>
                    {r && r.elapsed_ms > 0 && (
                      <span className="text-xs text-muted-foreground">
                        {r.elapsed_ms}ms
                      </span>
                    )}
                  </div>
                  {r?.message && (
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      {r.message}
                    </p>
                  )}
                </div>
              </li>
            )
          })}
        </ul>
        <DialogFooter>
          <Button onClick={onClose}>
            {allSuccess ? t("test.done") : t("test.close")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function StageIcon({
  status,
}: {
  status: ConnectionTestStageResult["status"]
}) {
  const cls = "h-4 w-4 mt-0.5"
  switch (status) {
    case "success":
      return <CheckCircle2 className={`${cls} text-green-600`} />
    case "failure":
      return <XCircle className={`${cls} text-destructive`} />
    case "running":
      return <Loader2 className={`${cls} animate-spin text-muted-foreground`} />
    case "skipped":
      return (
        <span
          className={`${cls} flex items-center justify-center text-muted-foreground`}
        >
          —
        </span>
      )
    case "pending":
    default:
      return (
        <span
          className={`${cls} flex items-center justify-center text-muted-foreground`}
        >
          ○
        </span>
      )
  }
}

// ── SSH config import tab ────────────────────────────────────────────────

function SshConfigImportTab({ onImported }: { onImported: () => void }) {
  const t = useTranslations("SshConnectionSettings")
  const [aliases, setAliases] = useState<SshConfigEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [importing, setImporting] = useState<string | null>(null)

  useEffect(() => {
    listSshConfigAliases()
      .then(setAliases)
      .catch((e) => {
        console.error(e)
        toast.error(t("sshConfig.loadFailed"))
      })
      .finally(() => setLoading(false))
  }, [t])

  const handleImport = useCallback(
    async (entry: SshConfigEntry) => {
      setImporting(entry.alias)
      try {
        const input: ConnectionInput = {
          name: entry.alias,
          kind: "ssh",
          ssh_host: entry.host,
          ssh_user: entry.user,
          ssh_port: entry.port,
          ssh_alias: entry.alias,
          ssh_key_path: entry.identity_file,
          ssh_auth_method: "ssh_config",
          proxy_jump: entry.proxy_jump,
          daemon_path: null,
          auto_connect: false,
        }
        await createConnection({ input })
        toast.success(t("sshConfig.imported", { alias: entry.alias }))
        onImported()
      } catch (e) {
        console.error(e)
        toast.error(t("sshConfig.importFailed"))
      } finally {
        setImporting(null)
      }
    },
    [onImported, t]
  )

  if (loading) {
    return (
      <div className="flex items-center justify-center py-8 text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  if (aliases.length === 0) {
    return (
      <div className="rounded-md border border-dashed p-8 text-center text-sm text-muted-foreground">
        {t("sshConfig.empty")}
      </div>
    )
  }

  return (
    <ul className="space-y-2">
      {aliases.map((a) => (
        <li
          key={a.alias}
          className="flex items-center justify-between gap-3 rounded-md border p-3"
        >
          <div className="min-w-0 flex-1">
            <div className="font-medium">{a.alias}</div>
            <div className="truncate text-xs text-muted-foreground">
              {a.user ? `${a.user}@` : ""}
              {a.host ?? "?"}
              {a.port && a.port !== 22 ? `:${a.port}` : ""}
              {a.proxy_jump ? ` (jump: ${a.proxy_jump})` : ""}
            </div>
          </div>
          <Button
            size="sm"
            variant="outline"
            disabled={importing === a.alias}
            onClick={() => handleImport(a)}
          >
            {importing === a.alias && (
              <Loader2 className="mr-1 h-4 w-4 animate-spin" />
            )}
            {t("sshConfig.import")}
          </Button>
        </li>
      ))}
    </ul>
  )
}
