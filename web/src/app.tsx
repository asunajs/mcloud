import { ref } from 'vue'
import { Dashboard } from './pages/dashboard'
import { Tasks } from './pages/tasks'

type Page = 'dashboard' | 'tasks'

/**
 * 应用外壳。
 *
 * 页面切换用局部状态 + 条件渲染，不引路由库 —— 当前页面数很少，
 * 路由库带来的收益不抵它的体积与心智负担。
 */
export default function App() {
  const page = ref<Page>('tasks')

  return (
    <div class="min-h-screen bg-base-200">
      <div class="navbar bg-base-100 shadow-sm">
        <div class="flex-1">
          <span class="text-lg font-semibold px-2">mcloud-gost</span>
        </div>
        <div class="flex-none">
          <ul class="menu menu-horizontal px-1">
            <li>
              <a
                class={page.value === 'tasks' ? 'active' : ''}
                onClick={() => (page.value = 'tasks')}
              >
                任务
              </a>
            </li>
            <li>
              <a
                class={page.value === 'dashboard' ? 'active' : ''}
                onClick={() => (page.value = 'dashboard')}
              >
                概览
              </a>
            </li>
          </ul>
        </div>
      </div>

      <main class="p-6">
        {page.value === 'tasks' ? <Tasks /> : <Dashboard />}
      </main>
    </div>
  )
}
