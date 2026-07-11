"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { toast } from "sonner"
import {
  ArrowLeft,
  ArrowRight,
  RotateCw,
  Home,
  Star,
  StarOff,
  Smartphone,
  Network,
  Globe,
  Loader2,
  X as XIcon,
  Bookmark,
  BookmarkX,
  Plus,
  Pencil,
  Trash2,
  Maximize2,
  Minimize2,
  ServerCog,
} from "lucide-react"
import {
  listBookmarks,
  upsertBookmark,
  deleteBookmark,
  listRemoteDevices,
  listDeviceServices,
  buildBrowseUrl,
  type BookmarkInfo,
  type DeviceServiceInfo,
  type RemoteDeviceMasked,
} from "@/lib/api"
import {
  getActiveDeviceId,
  setActiveDeviceId,
  subscribeActiveDevice,
} from "@/lib/transport"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"

const HOME_URL = "https://duckduckgo.com/?q="
const BOOKMARK_BAR_VISIBLE_KEY = "codeg_browser_bookmark_bar_visible"

interface BrowserTab {
  id: string
  title: string
  currentUrl: string | null
  history: string[]
  histPos: number
  /** bump 让 iframe re-mount */
  iframeKey: number
}

function newTab(url: string | null = null): BrowserTab {
  return {
    id: Math.random().toString(36).slice(2),
    title: url ?? "新标签页",
    currentUrl: url,
    history: url ? [url] : [],
    histPos: url ? 0 : -1,
    iframeKey: 0,
  }
}

function normalizeUrl(input: string): string {
  const t = input.trim()
  if (!t) return ""
  if (/^https?:\/\//i.test(t)) return t
  if (/^[\w.-]+\.[a-z]{2,}([\/?#].*)?$/i.test(t)) return "https://" + t
  return HOME_URL + encodeURIComponent(t)
}

function unwrapBrowseUrl(href: string): string | null {
  // 把 /api/browse?url=... 提回原 URL
  if (!href.includes("/api/browse?")) return null
  try {
    const u = new URL(href, window.location.origin)
    return u.searchParams.get("url")
  } catch {
    return null
  }
}

export function BrowserView() {
  const [tabs, setTabs] = useState<BrowserTab[]>([newTab(null)])
  const [activeTabId, setActiveTabId] = useState<string>(() => tabs[0]?.id ?? "")
  const [input, setInput] = useState("")
  const [bookmarks, setBookmarks] = useState<BookmarkInfo[]>([])
  const [devices, setDevices] = useState<RemoteDeviceMasked[]>([])
  const [activeDevice, setActiveDevice] = useState<number | null>(null)
  const [deviceServices, setDeviceServices] = useState<DeviceServiceInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [showBookmarkBar, setShowBookmarkBar] = useState(true)
  const [fullscreen, setFullscreen] = useState(false)
  const [bookmarkDialog, setBookmarkDialog] = useState<{
    open: boolean
    editing: BookmarkInfo | null
    title: string
    url: string
  }>({ open: false, editing: null, title: "", url: "" })
  const iframeRef = useRef<HTMLIFrameElement>(null)

  const activeTab = tabs.find((t) => t.id === activeTabId)

  // 当前 active tab url 跟随到 input
  useEffect(() => {
    setInput(activeTab?.currentUrl ?? "")
  }, [activeTabId, activeTab?.currentUrl])

  const refreshBookmarks = useCallback(async () => {
    try {
      setBookmarks(await listBookmarks())
    } catch (e) {
      console.error("[browser] list bookmarks", e)
    }
  }, [])

  const refreshDevices = useCallback(async () => {
    try {
      setDevices(await listRemoteDevices())
    } catch (e) {
      console.error("[browser] list devices", e)
    }
  }, [])

  const refreshDeviceServices = useCallback(async (deviceId: number | null) => {
    if (deviceId == null) {
      setDeviceServices([])
      return
    }
    try {
      const list = await listDeviceServices(deviceId)
      setDeviceServices(list.filter((s) => s.enabled))
    } catch (e) {
      console.error("[browser] list device services", e)
      setDeviceServices([])
    }
  }, [])

  useEffect(() => {
    Promise.all([refreshBookmarks(), refreshDevices()]).finally(() => setLoading(false))
    setActiveDevice(getActiveDeviceId())
    if (typeof window !== "undefined") {
      const saved = window.localStorage.getItem(BOOKMARK_BAR_VISIBLE_KEY)
      if (saved === "0") setShowBookmarkBar(false)
    }
    refreshDeviceServices(getActiveDeviceId())
    const unsub = subscribeActiveDevice((id) => {
      setActiveDevice(id)
      refreshDeviceServices(id)
      // 切设备后, 当前 tab 重新加载 (出口变了)
      setTabs((prev) =>
        prev.map((t) =>
          t.id === activeTabId && t.currentUrl
            ? { ...t, iframeKey: t.iframeKey + 1 }
            : t
        )
      )
    })
    return unsub
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // iframe 内点击 <a target="_blank"> 时, 拦截 → 新 tab 打开
  // postMessage 由我们注入到 rewritten HTML 里? 不,直接监听 iframe.contentDocument 更稳。
  // 但只有 same-origin (反代后是) 才能访问 contentDocument。
  const handleIframeLoad = useCallback(() => {
    const iframe = iframeRef.current
    if (!iframe) return
    let doc: Document | null = null
    try {
      doc = iframe.contentDocument
    } catch {
      return
    }
    if (!doc) return

    // 同步 tab title
    try {
      const t = doc.title
      if (t) {
        setTabs((prev) =>
          prev.map((tab) => (tab.id === activeTabId ? { ...tab, title: t } : tab))
        )
      }
    } catch {}

    const onClick = (e: Event) => {
      const target = e.target as HTMLElement | null
      const a = target?.closest?.("a") as HTMLAnchorElement | null
      if (!a) return
      const targetAttr = (a.getAttribute("target") ?? "").toLowerCase()
      const me = e as MouseEvent
      const wantNewTab =
        targetAttr === "_blank" ||
        // 中键点击 / Ctrl/Cmd-click: 也走内部新 tab
        (me && (me.button === 1 || me.ctrlKey || me.metaKey))
      // 其他 target (_top _parent _new 或具体 name) — server rewrite 已统一成 _blank,
      // 这里兜底再过一遍。但 sandbox 没 allow-top-navigation 已防 top.location 跳走。
      if (wantNewTab) {
        e.preventDefault()
        e.stopPropagation()
        const href = a.getAttribute("href") ?? ""
        const orig = unwrapBrowseUrl(href) ?? href
        if (orig) openInNewTab(orig)
      }
    }
    doc.addEventListener("click", onClick, true)
    // 兜底中键点击 (有些浏览器不触发 click)
    doc.addEventListener("auxclick", onClick, true)
    // window.open 拦截 — 由于 iframe sandbox 不带 allow-popups-to-escape-sandbox 已限制, 但加一层兜底
    try {
      const win = iframe.contentWindow as Window | null
      if (win) {
        win.open = ((url?: string | URL) => {
          if (!url) return null
          const s = String(url)
          const orig = unwrapBrowseUrl(s) ?? s
          openInNewTab(orig)
          return null
        }) as typeof window.open
      }
    } catch {}
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTabId])

  const openInNewTab = useCallback((rawUrl: string) => {
    const url = normalizeUrl(rawUrl)
    if (!url) return
    const tab = newTab(url)
    setTabs((prev) => [...prev, tab])
    setActiveTabId(tab.id)
  }, [])

  const toggleBookmarkBar = () => {
    setShowBookmarkBar((v) => {
      const next = !v
      if (typeof window !== "undefined") {
        window.localStorage.setItem(BOOKMARK_BAR_VISIBLE_KEY, next ? "1" : "0")
      }
      return next
    })
  }

  const go = useCallback(
    (rawUrl: string, replace = false) => {
      const url = normalizeUrl(rawUrl)
      if (!url) return
      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== activeTabId) return t
          if (replace && t.histPos >= 0) {
            const copy = [...t.history]
            copy[t.histPos] = url
            return { ...t, history: copy, currentUrl: url, iframeKey: t.iframeKey + 1 }
          }
          const trimmed = t.history.slice(0, t.histPos + 1)
          trimmed.push(url)
          return {
            ...t,
            history: trimmed,
            histPos: t.histPos + 1,
            currentUrl: url,
            iframeKey: t.iframeKey + 1,
          }
        })
      )
      setInput(url)
    },
    [activeTabId]
  )

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    go(input)
  }

  const updateActive = (patch: (t: BrowserTab) => BrowserTab) => {
    setTabs((prev) => prev.map((t) => (t.id === activeTabId ? patch(t) : t)))
  }

  const handleBack = () => {
    if (!activeTab || activeTab.histPos <= 0) return
    updateActive((t) => ({
      ...t,
      histPos: t.histPos - 1,
      currentUrl: t.history[t.histPos - 1],
      iframeKey: t.iframeKey + 1,
    }))
  }

  const handleForward = () => {
    if (!activeTab || activeTab.histPos >= activeTab.history.length - 1) return
    updateActive((t) => ({
      ...t,
      histPos: t.histPos + 1,
      currentUrl: t.history[t.histPos + 1],
      iframeKey: t.iframeKey + 1,
    }))
  }

  const handleReload = () => {
    if (!activeTab?.currentUrl) return
    updateActive((t) => ({ ...t, iframeKey: t.iframeKey + 1 }))
  }

  const handleHome = () => {
    updateActive((t) => ({ ...t, currentUrl: null, title: "新标签页" }))
  }

  const isBookmarked =
    activeTab?.currentUrl && bookmarks.some((b) => b.url === activeTab.currentUrl)

  const toggleBookmark = async () => {
    if (!activeTab?.currentUrl) return
    const existing = bookmarks.find((b) => b.url === activeTab.currentUrl)
    if (existing) {
      await deleteBookmark(existing.id)
      toast.success("已取消收藏")
    } else {
      await upsertBookmark({ title: activeTab.title, url: activeTab.currentUrl })
      toast.success("已收藏")
    }
    refreshBookmarks()
  }

  const switchDevice = (deviceId: number | null) => {
    setActiveDeviceId(deviceId)
    setActiveDevice(deviceId)
    updateActive((t) => ({ ...t, iframeKey: t.iframeKey + 1 }))
  }

  const closeTab = (id: string) => {
    setTabs((prev) => {
      const idx = prev.findIndex((t) => t.id === id)
      if (idx < 0) return prev
      const next = prev.filter((t) => t.id !== id)
      if (next.length === 0) return [newTab(null)]
      // 关闭 active 时,选最近一个
      if (id === activeTabId) {
        const newActive = next[Math.min(idx, next.length - 1)]
        setActiveTabId(newActive.id)
      }
      return next
    })
  }

  const addBlankTab = () => {
    const t = newTab(null)
    setTabs((prev) => [...prev, t])
    setActiveTabId(t.id)
  }

  const openBookmarkDialog = (b: BookmarkInfo | null) => {
    setBookmarkDialog({
      open: true,
      editing: b,
      title: b?.title ?? "",
      url: b?.url ?? "",
    })
  }

  const saveBookmark = async () => {
    if (!bookmarkDialog.url.trim()) {
      toast.error("URL 不能为空")
      return
    }
    try {
      await upsertBookmark({
        id: bookmarkDialog.editing?.id,
        title: bookmarkDialog.title.trim() || bookmarkDialog.url.trim(),
        url: bookmarkDialog.url.trim(),
      })
      setBookmarkDialog({ open: false, editing: null, title: "", url: "" })
      refreshBookmarks()
      toast.success("已保存")
    } catch (e) {
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) })
    }
  }

  const deleteFromList = async (b: BookmarkInfo) => {
    await deleteBookmark(b.id)
    refreshBookmarks()
  }

  const activeDeviceName = activeDevice
    ? devices.find((d) => d.id === activeDevice)?.name ?? `设备 ${activeDevice}`
    : "本机"

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  // 全屏模式: 整个 BrowserView 升到 fixed top-level, 覆盖 workspace top bar.
  // 左下角浮动一个半透明 button 退出全屏。
  if (fullscreen) {
    return (
      <div className="fixed inset-0 z-[100] bg-background">
        {activeTab?.currentUrl ? (
          <iframe
            key={`${activeTab.id}-${activeTab.iframeKey}`}
            ref={iframeRef}
            src={buildBrowseUrl(activeTab.currentUrl, activeDevice)}
            onLoad={handleIframeLoad}
            className="w-full h-full border-0"
            sandbox="allow-forms allow-scripts allow-same-origin allow-downloads"
            referrerPolicy="no-referrer"
          />
        ) : (
          <div className="flex flex-col items-center justify-center h-full gap-3 p-8">
            <Globe className="h-12 w-12 text-muted-foreground/40" />
            <p className="text-sm text-muted-foreground">无活动页面 — 退出全屏后输入网址</p>
          </div>
        )}
        <button
          onClick={() => setFullscreen(false)}
          className="fixed bottom-4 left-4 z-[101] h-10 w-10 rounded-full bg-foreground/30 hover:bg-foreground/60 text-background backdrop-blur-sm flex items-center justify-center shadow-lg transition-colors"
          title="退出全屏"
          aria-label="退出全屏"
        >
          <Minimize2 className="h-4 w-4" />
        </button>
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full bg-background">
      {/* Tab bar */}
      <div className="flex items-stretch gap-px border-b bg-muted/30 px-1 pt-1 shrink-0 overflow-x-auto">
        {tabs.map((t) => {
          const isActive = t.id === activeTabId
          return (
            <div
              key={t.id}
              onClick={() => setActiveTabId(t.id)}
              className={
                "group flex items-center gap-1.5 px-2 py-1.5 cursor-pointer max-w-[200px] rounded-t-md text-xs select-none " +
                (isActive
                  ? "bg-background border-x border-t border-border text-foreground"
                  : "text-muted-foreground hover:bg-background/60")
              }
            >
              <Globe className="h-3 w-3 shrink-0 opacity-70" />
              <span className="truncate flex-1">{t.title || "新标签页"}</span>
              {tabs.length > 1 && (
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    closeTab(t.id)
                  }}
                  className="opacity-40 hover:opacity-100 hover:text-destructive"
                  title="关闭标签页"
                >
                  <XIcon className="h-3 w-3" />
                </button>
              )}
            </div>
          )
        })}
        <button
          onClick={addBlankTab}
          className="px-2 py-1.5 text-muted-foreground hover:text-foreground"
          title="新标签页"
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* 顶部导航栏 — 窄屏(手机竖屏)地址栏单独换行占满, md+ (平板/电脑) 单行 */}
      <div className="flex flex-wrap items-center gap-1.5 border-b px-2 py-1.5 shrink-0">
        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={handleBack} disabled={!activeTab || activeTab.histPos <= 0}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8"
          onClick={handleForward}
          disabled={!activeTab || activeTab.histPos >= activeTab.history.length - 1}
        >
          <ArrowRight className="h-4 w-4" />
        </Button>
        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={handleReload} disabled={!activeTab?.currentUrl}>
          <RotateCw className="h-4 w-4" />
        </Button>
        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={handleHome}>
          <Home className="h-4 w-4" />
        </Button>

        <form
          onSubmit={handleSubmit}
          className="order-last w-full flex md:order-none md:w-auto md:flex-1"
        >
          <Input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="输入网址或搜索词"
            className="h-8 text-sm"
          />
        </form>

        {activeTab?.currentUrl && (
          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={toggleBookmark}>
            {isBookmarked ? <Star className="h-4 w-4 fill-yellow-400 text-yellow-500" /> : <StarOff className="h-4 w-4" />}
          </Button>
        )}

        {/* 书签栏开关 */}
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8"
          onClick={toggleBookmarkBar}
          title={showBookmarkBar ? "隐藏书签栏" : "显示书签栏"}
        >
          {showBookmarkBar ? <BookmarkX className="h-4 w-4" /> : <Bookmark className="h-4 w-4" />}
        </Button>

        {/* 全屏 */}
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8"
          onClick={() => setFullscreen(true)}
          title="进入全屏 (隐藏顶部工具栏)"
        >
          <Maximize2 className="h-4 w-4" />
        </Button>

        {/* 书签管理 — 放在出口设备的左边 */}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" className="h-8 w-8" title="书签管理">
              <Bookmark className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-64">
            <DropdownMenuLabel className="text-xs flex items-center justify-between">
              所有收藏
              <Button
                size="sm"
                variant="ghost"
                className="h-6 px-1.5 text-xs"
                onClick={() => openBookmarkDialog(null)}
              >
                <Plus className="h-3 w-3 mr-0.5" /> 新建
              </Button>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            {bookmarks.length === 0 ? (
              <DropdownMenuItem disabled className="text-xs">
                还没有收藏
              </DropdownMenuItem>
            ) : (
              bookmarks.map((b) => (
                <div key={b.id} className="flex items-center gap-1 group hover:bg-accent px-1">
                  <DropdownMenuItem
                    onClick={() => go(b.url)}
                    className="flex-1 flex flex-col items-start py-1.5 hover:bg-transparent focus:bg-transparent"
                  >
                    <span className="text-sm truncate">{b.title}</span>
                    <span className="text-[10px] text-muted-foreground truncate w-full font-mono">{b.url}</span>
                  </DropdownMenuItem>
                  <button
                    onClick={(e) => {
                      e.stopPropagation()
                      openBookmarkDialog(b)
                    }}
                    className="p-1 opacity-0 group-hover:opacity-100"
                  >
                    <Pencil className="h-3 w-3" />
                  </button>
                  <button
                    onClick={(e) => {
                      e.stopPropagation()
                      deleteFromList(b)
                    }}
                    className="p-1 opacity-0 group-hover:opacity-100 text-destructive"
                  >
                    <Trash2 className="h-3 w-3" />
                  </button>
                </div>
              ))
            )}
          </DropdownMenuContent>
        </DropdownMenu>

        {/* 设备内网服务快捷下拉 — 只在切到 device 且有 enabled 服务时显示 */}
        {activeDevice != null && deviceServices.length > 0 && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" className="h-8 w-8" title={`${activeDeviceName} 的内网服务`}>
                <ServerCog className="h-4 w-4 text-emerald-600" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-60">
              <DropdownMenuLabel className="text-xs">{activeDeviceName} 的内网服务</DropdownMenuLabel>
              <DropdownMenuSeparator />
              {deviceServices.map((s) => (
                <DropdownMenuItem key={s.id} onClick={() => go(s.url)}>
                  <Globe className="h-3.5 w-3.5 mr-2 text-muted-foreground" />
                  <div className="flex flex-col min-w-0">
                    <span className="truncate">{s.name}</span>
                    <span className="text-[10px] text-muted-foreground font-mono truncate">{s.url}</span>
                  </div>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}

        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="sm" className="h-8 gap-1 text-xs">
              {activeDevice == null ? (
                <Smartphone className="h-3.5 w-3.5" />
              ) : (
                <Network className="h-3.5 w-3.5 text-emerald-600" />
              )}
              {activeDeviceName}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-56">
            <DropdownMenuLabel className="text-xs">浏览器出口设备</DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => switchDevice(null)}>
              <Smartphone className="h-3.5 w-3.5 mr-2" /> 本机
            </DropdownMenuItem>
            {devices.map((d) => (
              <DropdownMenuItem key={d.id} onClick={() => switchDevice(d.id)}>
                <Network className="h-3.5 w-3.5 mr-2 text-emerald-600" />
                <div className="flex flex-col">
                  <span>{d.name}</span>
                  <span className="text-[10px] text-muted-foreground font-mono">{d.base_url}</span>
                </div>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {/* 书签栏 (横向 scrollable, 可隐藏) */}
      {showBookmarkBar && bookmarks.length > 0 && (
        <div className="flex items-center gap-1 overflow-x-auto border-b px-2 py-1 shrink-0 text-xs">
          {bookmarks.slice(0, 20).map((b) => (
            <button
              key={b.id}
              onClick={() => go(b.url)}
              className="shrink-0 flex items-center gap-1 rounded hover:bg-accent px-1.5 py-1 max-w-[140px]"
              title={b.url}
            >
              <Globe className="h-3 w-3 shrink-0 text-muted-foreground" />
              <span className="truncate">{b.title}</span>
            </button>
          ))}
        </div>
      )}

      {/* iframe 或空状态 */}
      <div className="flex-1 min-h-0 relative">
        {activeTab?.currentUrl ? (
          <iframe
            key={`${activeTab.id}-${activeTab.iframeKey}`}
            ref={iframeRef}
            src={buildBrowseUrl(activeTab.currentUrl, activeDevice)}
            onLoad={handleIframeLoad}
            className="w-full h-full border-0"
            sandbox="allow-forms allow-scripts allow-same-origin allow-downloads"
            referrerPolicy="no-referrer"
          />
        ) : (
          <div className="flex flex-col items-center justify-center h-full gap-3 p-8">
            <Globe className="h-12 w-12 text-muted-foreground/40" />
            <p className="text-sm text-muted-foreground">输入网址或搜索词,或者点击收藏</p>
            <p className="text-xs text-muted-foreground">
              出口设备: {activeDevice == null ? "本机" : activeDeviceName}
            </p>
          </div>
        )}
      </div>

      <Dialog open={bookmarkDialog.open} onOpenChange={(o) => !o && setBookmarkDialog({ ...bookmarkDialog, open: false })}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{bookmarkDialog.editing ? "编辑收藏" : "新建收藏"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-3 py-2">
            <div className="space-y-1.5">
              <Label>标题</Label>
              <Input
                value={bookmarkDialog.title}
                onChange={(e) => setBookmarkDialog({ ...bookmarkDialog, title: e.target.value })}
                placeholder="(可选, 留空用 URL)"
              />
            </div>
            <div className="space-y-1.5">
              <Label>URL</Label>
              <Input
                value={bookmarkDialog.url}
                onChange={(e) => setBookmarkDialog({ ...bookmarkDialog, url: e.target.value })}
                placeholder="https://example.com"
              />
            </div>
          </div>
          <DialogFooter>
            <Button
              variant="ghost"
              onClick={() => setBookmarkDialog({ open: false, editing: null, title: "", url: "" })}
            >
              <XIcon className="h-3.5 w-3.5 mr-1" /> 取消
            </Button>
            <Button onClick={saveBookmark}>保存</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
