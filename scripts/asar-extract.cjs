#!/usr/bin/env node
// 从 Electron 的 app.asar 里列出 / 抽取文件。
//
// 为什么自己写而不是用 `npx @electron/asar`：
// 本项目的门禁要求"不联网也能跑"，而 npx 会去下载。asar 的格式其实很小，
// 头部就是一个 JSON 目录树，自己解析比引依赖更可控。
//
// 用法：
//   node scripts/asar-extract.cjs list  <app.asar> [子串过滤]
//   node scripts/asar-extract.cjs cat   <app.asar> <内部路径>
//   node scripts/asar-extract.cjs grep  <app.asar> <正则> [只搜 .js]
//   node scripts/asar-extract.cjs save  <app.asar> <内部路径> <输出文件>

'use strict'

const fs = require('node:fs')
const path = require('node:path')

/**
 * asar 头部布局（小端）—— 实测于 `/Applications/中国移动云盘.app`：
 *   [0..3]   uint32 = 4              —— 后面那个长度字段本身的字节数
 *   [4..7]   uint32 = size           —— 头部 pickle 的字节数（含它自己的长度前缀）
 *   [8..11]  uint32 = 头部 pickle 的载荷长度
 *   [12..15] uint32 = jsonLen        —— JSON 字符串的字节数
 *   [16..]   JSON 目录树
 * 文件内容区从 8 + size 开始，按目录树里的 offset/size 切片。
 *
 * ⚠ 别想当然地按"JSON 紧跟 8 字节"去切 —— 这里多了 4 字节的载荷长度前缀。
 * 自检：`JSON 起始 + jsonLen == 8 + size`，对不上说明偏移猜错了。
 */
function readArchive(asarPath) {
  // 参数顺序是 `<子命令> <app.asar> <参数…>`。踩过一次：把子命令的实参写在 asar 前面
  // （`grep <正则> <app.asar>`），于是拿正则当路径去 open —— 报错是 ENOENT，
  // 但**没有**任何提示说"你参数顺序反了"。这里补一条，让下次一眼看出来。
  if (!fs.existsSync(asarPath)) {
    console.error(
      `找不到 asar：${asarPath}\n` +
        `（若你把子命令的实参写在前面了，注意顺序是 <子命令> <app.asar> <参数…>，\n` +
        `  见本文件头部的用法注释）`,
    )
    process.exit(2)
  }
  const fd = fs.openSync(asarPath, 'r')
  try {
    const sizeBuf = Buffer.alloc(8)
    fs.readSync(fd, sizeBuf, 0, 8, 0)
    const size = sizeBuf.readUInt32LE(4)

    const headerBuf = Buffer.alloc(size)
    fs.readSync(fd, headerBuf, 0, size, 8)
    const jsonLen = headerBuf.readUInt32LE(4)
    const json = headerBuf.subarray(8, 8 + jsonLen).toString('utf8')

    const baseOffset = 8 + size
    if (16 + jsonLen !== baseOffset) {
      throw new Error(
        `头部偏移自检失败：JSON 起始 16 + jsonLen ${jsonLen} != 内容区起点 ${baseOffset}`,
      )
    }

    return { fd, tree: JSON.parse(json), baseOffset }
  } catch (err) {
    fs.closeSync(fd)
    throw err
  }
}

/** 把目录树展平成 [内部路径, {offset,size}] 列表。 */
function walk(node, prefix, out) {
  for (const [name, entry] of Object.entries(node.files || {})) {
    const p = prefix ? `${prefix}/${name}` : name
    if (entry.files) walk(entry, p, out)
    else if (typeof entry.offset === 'string') {
      out.push([p, { offset: Number(entry.offset), size: entry.size }])
    }
  }
  return out
}

function readEntry(archive, entry) {
  const buf = Buffer.alloc(entry.size)
  fs.readSync(archive.fd, buf, 0, entry.size, archive.baseOffset + entry.offset)
  return buf
}

function main() {
  const [cmd, asarPath, ...rest] = process.argv.slice(2)
  if (!cmd || !asarPath) {
    console.error('用法见文件头注释')
    return 2
  }
  const archive = readArchive(asarPath)
  try {
    const files = walk(archive.tree, '', [])

    if (cmd === 'list') {
      const filter = rest[0]
      const hits = files.filter(([p]) => !filter || p.includes(filter))
      for (const [p, e] of hits) console.log(`${String(e.size).padStart(10)}  ${p}`)
      console.log(`\n共 ${hits.length} / ${files.length} 个文件`)
      return 0
    }

    if (cmd === 'cat') {
      const target = files.find(([p]) => p === rest[0])
      if (!target) {
        console.error(`没找到：${rest[0]}`)
        return 1
      }
      process.stdout.write(readEntry(archive, target[1]))
      return 0
    }

    if (cmd === 'save') {
      const target = files.find(([p]) => p === rest[0])
      if (!target) {
        console.error(`没找到：${rest[0]}`)
        return 1
      }
      fs.mkdirSync(path.dirname(rest[1]), { recursive: true })
      fs.writeFileSync(rest[1], readEntry(archive, target[1]))
      console.log(`已写出 ${rest[1]}（${target[1].size} 字节）`)
      return 0
    }

    if (cmd === 'grep') {
      const re = new RegExp(rest[0], 'i')
      const onlyJs = rest[1] !== '--all'
      let hits = 0
      for (const [p, e] of files) {
        if (onlyJs && !/\.(js|mjs|cjs|json|ts)$/.test(p)) continue
        if (e.size > 4 * 1024 * 1024) continue // 跳过巨型文件
        const text = readEntry(archive, e).toString('utf8')
        const lines = text.split('\n')
        lines.forEach((line, i) => {
          if (re.test(line)) {
            console.log(`${p}:${i + 1}: ${line.trim().slice(0, 200)}`)
            hits += 1
          }
        })
      }
      console.log(`\n命中 ${hits} 行`)
      return 0
    }

    console.error(`未知子命令：${cmd}`)
    return 2
  } finally {
    fs.closeSync(archive.fd)
  }
}

process.exit(main())
