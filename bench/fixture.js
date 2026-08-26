// Build a throwaway repo for one benchmark case, then re-stage it between timed runs.
//
//   node fixture.js build <dir> <repo-files> <staged> <clean|partial>
//   node fixture.js stage <dir> <staged> <clean|partial>
//
// `build` creates and commits `repo-files` files. `stage` returns the repo to its committed
// state and restages `staged` of them, so hyperfine's --prepare costs O(staged), not O(repo).

import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

const TASK = { '*.txt': 'true' }
const LEFTHOOK = {
  'pre-commit': {
    jobs: [{ name: 'task', glob: '*.txt', stage_fixed: true, run: `true {staged_files}` }],
  },
}
const PRE_COMMIT = {
  repos: [
    {
      repo: 'local',
      hooks: [{ id: 'task', name: 'task', language: 'system', entry: 'true', files: '\\.txt$' }],
    },
  ],
}

const git = (dir, ...args) =>
  execFileSync('git', args, {
    cwd: dir,
    stdio: 'pipe',
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' },
  })

const name = (i) => `f${String(i).padStart(6, '0')}.txt`

function build(dir, repoFiles, staged, mode) {
  fs.rmSync(dir, { recursive: true, force: true })
  fs.mkdirSync(dir, { recursive: true })

  git(dir, 'init', '-q', '.')
  git(dir, 'config', 'user.email', 'bench@example.com')
  git(dir, 'config', 'user.name', 'bench')
  // Auto-gc forks a background repack that would land in the middle of a timed run.
  git(dir, 'config', 'gc.auto', '0')
  // Competitors resolve config differently; give each the file it looks for.
  fs.writeFileSync(path.join(dir, '.stagelint.json'), JSON.stringify(TASK))
  fs.writeFileSync(path.join(dir, '.lintstagedrc.json'), JSON.stringify(TASK))
  fs.writeFileSync(path.join(dir, '.nano-staged.json'), JSON.stringify(TASK))
  fs.writeFileSync(path.join(dir, 'lefthook.json'), JSON.stringify(LEFTHOOK))
  fs.writeFileSync(path.join(dir, '.pre-commit-config.yaml'), JSON.stringify(PRE_COMMIT))

  for (let i = 0; i < repoFiles; i++) {
    fs.writeFileSync(path.join(dir, name(i)), `committed ${i}\n`)
  }
  git(dir, 'add', '-A')
  git(dir, 'commit', '-qm', 'fixture')
  // Real repos are packed; loose objects turn every existence probe into a stat.
  git(dir, 'repack', '-adq')

  stage(dir, staged, mode)
}

function stage(dir, staged, mode) {
  // Tools leave behind stash refs and patch files; a dirty carry-over would skew the next run.
  git(dir, 'reset', '-q', '--hard', 'HEAD')
  git(dir, 'clean', '-qfd')
  git(dir, 'stash', 'clear')
  for (const stray of ['nano-staged.patch', 'nano-staged_partial.patch']) {
    fs.rmSync(path.join(dir, '.git', stray), { force: true })
  }

  // Novel content per run; restaging identical bytes rebuilds a tree the ODB already holds.
  const nonce = process.hrtime.bigint().toString(36)

  const files = []
  for (let i = 0; i < staged; i++) {
    const file = name(i)
    fs.writeFileSync(path.join(dir, file), `staged ${i} ${nonce}\n`)
    files.push(file)
  }
  if (files.length) {
    git(dir, 'add', '--', ...files)
  }

  // Partial staging is the case every tool must hide and restore around the run.
  if (mode === 'partial') {
    for (let i = 0; i < staged; i++) {
      fs.writeFileSync(path.join(dir, name(i)), `staged ${i} ${nonce}\nunstaged ${i} ${nonce}\n`)
    }
  }
}

const [cmd, dir, ...rest] = process.argv.slice(2)
if (cmd === 'build') {
  build(path.resolve(dir), Number(rest[0]), Number(rest[1]), rest[2])
} else if (cmd === 'stage') {
  stage(path.resolve(dir), Number(rest[0]), rest[1])
} else {
  console.error('usage: fixture.js build|stage <dir> ...')
  process.exit(2)
}
