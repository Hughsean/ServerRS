export function formatDate(value?: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat('zh-CN', {
        year: 'numeric',
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      }).format(date)
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat('zh-CN').format(value)
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : '请求失败，请稍后重试'
}

export function statusTone(status?: string | null): string {
  const normalized = status?.toLowerCase()
  if (['active', 'published', 'completed', 'success', 'low', 'none'].includes(normalized ?? '')) {
    return 'success'
  }
  if (['failed', 'disabled', 'critical', 'high'].includes(normalized ?? '')) return 'danger'
  if (['staged', 'pending', 'processing', 'medium'].includes(normalized ?? '')) return 'warning'
  return 'neutral'
}
