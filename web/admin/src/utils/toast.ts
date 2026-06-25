/**
 * Toast 通知状态管理
 *
 * 提供全局的 Toast 通知能力，支持 success/error/info/warning 四种类型。
 * 使用 Vue reactive 数组管理，由 ToastProvider 组件渲染。
 */

import { reactive } from 'vue'

export interface ToastMessage {
  id: number
  type: 'success' | 'error' | 'info' | 'warning'
  message: string
}

const toasts = reactive<ToastMessage[]>([])
let nextId = 1

export function useToast() {
  function show(type: ToastMessage['type'], message: string, duration = 3000) {
    const id = nextId++
    toasts.push({ id, type, message })
    setTimeout(() => {
      const idx = toasts.findIndex((t) => t.id === id)
      if (idx !== -1) toasts.splice(idx, 1)
    }, duration)
  }

  return {
    toasts,
    success: (msg: string) => show('success', msg),
    error: (msg: string) => show('error', msg),
    info: (msg: string) => show('info', msg),
    warning: (msg: string) => show('warning', msg),
  }
}
