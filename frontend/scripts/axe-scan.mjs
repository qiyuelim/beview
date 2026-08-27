// axe-core 无障碍扫描（AGENTS 基准 8 验收项：双主题 × 全页面）。
// 用法（仓库根）:
//   AXE_PASS='临时密码' node frontend/scripts/axe-scan.mjs
// 可选环境变量: AXE_BASE(默认 http://127.0.0.1:8765) AXE_USER(默认 axe_scan)
// 输出: 控制台按影响等级分组的违规报告 + logs/axe-report.json
import { chromium } from 'playwright'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'

const BASE = process.env.AXE_BASE || 'http://127.0.0.1:8765'
const USER = process.env.AXE_USER || 'axe_scan'
const PASS = process.env.AXE_PASS
if (!PASS) {
  console.error('缺少 AXE_PASS 环境变量')
  process.exit(1)
}

// 静态路由（Layout.tsx 路由表）；详情页在登录后按 API 实际数据补充
const ROUTES = [
  '/', '/review', '/review/wrong', '/drills', '/drills/new',
  '/resume', '/points', '/data', '/applications', '/companies',
  '/questions', '/skills', '/new', '/settings', '/settings/llm',
]

const THEMES = ['light', 'dark']
const IMPACTS = ['critical', 'serious', 'moderate', 'minor']

// 登录并返回已认证 context；登录后顺手收集可用的详情页路由
async function login(browser, theme) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } })
  await ctx.addInitScript((t) => localStorage.setItem('ir-theme', t), theme)
  const page = await ctx.newPage()
  await page.goto(`${BASE}/login`, { waitUntil: 'domcontentloaded' })
  await page.fill('#lg-user', USER)
  await page.fill('#lg-pass', PASS)
  await page.click('button[type="submit"]')
  await page.waitForURL(`${BASE}/`, { timeout: 10_000 })
  return { ctx, page }
}

async function collectDetailRoutes(page) {
  const pick = async (api, prefix) => {
    try {
      const res = await page.request.get(`${BASE}${api}`)
      if (!res.ok()) return []
      const arr = await res.json()
      const list = Array.isArray(arr) ? arr : arr.items || []
      return list.slice(0, 2).map((x) => `${prefix}/${x.id}`).filter((p) => !p.endsWith('/null'))
    } catch {
      return []
    }
  }
  // 数组端点直接取前两条；对象聚合端点取 .id 字段兜底
  const apps = await pick('/api/applications', '/applications')
  const companies = await pick('/api/companies', '/companies')
  const questions = await pick('/api/questions?page=1&page_size=5', '/questions')
  const drills = await pick('/api/drills', '/drills')
  let positions = []
  let rounds = []
  for (const a of apps.slice(0, 1)) {
    try {
      const d = await (await page.request.get(`${BASE}/api${a}`)).json()
      if (d?.position?.id) positions.push(`/positions/${d.position.id}`)
      if (d?.rounds?.[0]?.id) rounds.push(`/rounds/${d.rounds[0].id}`)
      else if (Array.isArray(d?.rounds) === false && d?.application) {
        // 兼容聚合形状差异，尽力而为
      }
    } catch {}
  }
  return [...apps, ...companies, ...questions, ...drills, ...positions, ...rounds]
}

const report = {}
let totalViolations = 0

const browser = await chromium.launch({ channel: 'chrome', headless: true })
try {
  for (const theme of THEMES) {
    const { ctx, page } = await login(browser, theme)
    if (!report[theme]) report[theme] = {}

    // 详情页路由只需收集一次（数据相同）
    if (theme === 'light') {
      const details = await collectDetailRoutes(page)
      ROUTES.push(...details.filter((r) => !ROUTES.includes(r)))
      console.error(`详情页路由: ${details.join(', ') || '（无数据，跳过）'}`)
    }

    for (const route of [...new Set(ROUTES)]) {
      try {
        await page.goto(`${BASE}${route}`, { waitUntil: 'domcontentloaded', timeout: 20_000 })
        await page.waitForTimeout(900) // 数据加载/SSE 建连缓冲
        await page.addScriptTag({ path: new URL('../node_modules/axe-core/axe.min.js', import.meta.url).pathname })
        const result = await page.evaluate(async () => {
          const r = await window.axe.run(document, {
            runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice'] },
          })
          return {
            url: location.pathname,
            violations: r.violations.map((v) => ({
              id: v.id,
              impact: v.impact,
              help: v.help,
              tags: v.tags.filter((t) => t.startsWith('wcag')),
              nodes: v.nodes.length,
              sample: v.nodes.slice(0, 3).map((n) => n.target.join(' ')),
            })),
            passes: r.passes.length,
          }
        })
        report[theme][result.url] = result
        const n = result.violations.reduce((s, v) => s + v.nodes, 0)
        totalViolations += n
        console.log(`[${theme}] ${result.url} — ${n} 节点违规 / ${result.violations.length} 规则`)
      } catch (e) {
        console.error(`[${theme}] ${route} 扫描失败: ${e.message.split('\n')[0]}`)
        report[theme][route] = { error: e.message.split('\n')[0] }
      }
    }
    await ctx.close()
  }
} finally {
  await browser.close()
}

// ---------- 汇总报告 ----------
console.log('\n================ AXE 汇总（按影响等级） ================')
const byRule = new Map()
for (const [theme, pages] of Object.entries(report)) {
  for (const [url, r] of Object.entries(pages)) {
    if (!r.violations) continue
    for (const v of r.violations) {
      const key = `${v.id}|${theme}`
      const e = byRule.get(key) || { theme, ...v, pages: [] }
      e.pages.push(url)
      byRule.set(key, e)
    }
  }
}
for (const impact of IMPACTS) {
  const items = [...byRule.values()].filter((v) => v.impact === impact)
  if (!items.length) continue
  console.log(`\n【${impact.toUpperCase()}】`)
  for (const v of items.sort((a, b) => b.nodes - a.nodes)) {
    console.log(`  • (${v.theme}) ${v.id} — ${v.help} [${v.tags.join(',')}]`)
    console.log(`    页面(${v.pages.length}): ${v.pages.slice(0, 6).join(', ')}${v.pages.length > 6 ? ' …' : ''}`)
    console.log(`    违规节点 ${v.nodes} 个，示例: ${v.sample[0] || '-'}`)
  }
}
console.log(`\n总违规节点数: ${totalViolations}`)

mkdirSync(new URL('../../logs', import.meta.url).pathname, { recursive: true })
writeFileSync(new URL('../../logs/axe-report.json', import.meta.url).pathname, JSON.stringify(report, null, 2))
console.log('完整报告: logs/axe-report.json')
