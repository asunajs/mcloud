/**
 * 设置页。
 *
 * 当前只有只读的实例信息 —— 账号管理与推送渠道要等后端接口就绪。
 * 空态按规范要求给出**下一步**，不写「暂无数据」。
 */

import { onMounted, ref } from 'lite'
import { getStatus, getVersion, type ServiceStatus, type VersionInfo } from '../api'

type LoadState = 'loading' | 'error' | 'ready'

export const SettingsPage = () => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const version = ref<VersionInfo | null>(null)
  const status = ref<ServiceStatus | null>(null)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [v, s] = await Promise.all([getVersion(), getStatus()])
      version.value = v
      status.value = s
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h1 class="text-base font-medium">设置</h1>
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
          disabled={state.value === 'loading'}
          onClick={() => void load()}
        >
          刷新
        </button>
      </div>

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <>
          <div class="card bg-base-100 shadow-sm">
            <div class="card-body">
              <h2 class="card-title text-base">实例信息</h2>
              <fieldset class="fieldset">
                <legend class="fieldset-legend">产品</legend>
                <input class="input w-full" value={version.value?.name ?? ''} readonly />
              </fieldset>
              <fieldset class="fieldset">
                <legend class="fieldset-legend">版本</legend>
                <input
                  class="input w-full"
                  value={version.value?.version ?? ''}
                  readonly
                />
              </fieldset>
              <fieldset class="fieldset">
                <legend class="fieldset-legend">已注册任务</legend>
                <input
                  class="input w-full"
                  value={String(status.value?.taskCount ?? 0)}
                  readonly
                />
              </fieldset>
            </div>
          </div>

          <div class="card bg-base-100 shadow-sm">
            <div class="card-body items-center gap-3 py-12 text-center">
              <span class="badge badge-soft badge-warning">待实现</span>
              <p class="text-sm opacity-70">账号管理与推送渠道还没接。</p>
              <p class="max-w-prose break-words text-xs opacity-60">
                这两块要等后端的账号与通知接口就绪。当前可在任务中心直接填账号运行任务。
              </p>
            </div>
          </div>
        </>
      ) : null}
    </div>
  )
}
