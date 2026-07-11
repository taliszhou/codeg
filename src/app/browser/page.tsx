"use client"

import { Suspense } from "react"
import { BrowserView } from "@/components/browser/browser-view"

export default function BrowserPage() {
  useEffect_setDocTitle()
  return (
    <Suspense>
      <div className="h-screen">
        <BrowserView />
      </div>
    </Suspense>
  )
}

function useEffect_setDocTitle() {
  if (typeof document !== "undefined") {
    document.title = "Browser - codeg"
  }
}
