// Turn a hyperfine parameter sweep into the README table: one row per staged count, one column
// per tool, each cell `clean / partial`. Everything is derived from `parameters`, so adding a
// value to a --parameter-list in run.sh widens the table with no change here.
//
//   node table.js results/bench.json

import fs from 'node:fs'

const [file] = process.argv.slice(2)
if (!file) {
  console.error('usage: table.js <hyperfine.json>')
  process.exit(2)
}

const { results } = JSON.parse(fs.readFileSync(file, 'utf8'))
const ms = (v) => (v === undefined ? '-' : `${Math.round(v * 1000).toLocaleString('en-US')}ms`)

const uniq = (key) => [...new Set(results.map((r) => r.parameters[key]))]
const tools = uniq('tool')
const rows = uniq('staged').sort((a, b) => a - b)
const modes = uniq('mode')

const mean = (tool, staged, mode) =>
  results.find(
    (r) =>
      r.parameters.tool === tool && r.parameters.staged === staged && r.parameters.mode === mode,
  )?.mean

const line = (cells) => `| ${cells.join(' | ')} |`
const out = [
  line(['Staged files', ...tools]),
  line(['---', ...tools.map(() => '---')]),
  ...rows.map((staged) =>
    line([staged, ...tools.map((t) => modes.map((m) => ms(mean(t, staged, m))).join(' / '))]),
  ),
]

const repoFiles = uniq('repo-files')
  .map((n) => Number(n).toLocaleString('en-US'))
  .join(', ')
out.push('')
out.push(
  `${modes.join(' / ')}. ${repoFiles}-file repo, no-op task, ${results[0].times.length} runs.`,
)

console.log(out.join('\n'))
