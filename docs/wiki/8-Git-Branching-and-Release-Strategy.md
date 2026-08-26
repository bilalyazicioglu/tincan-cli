# 8. Git Branching & Release Strategy

To ensure code stability, seamless community contributions, and predictable releases, **Tincan CLI** follows a structured **Git Flow / GitHub Flow hybrid model**.

---

## 8.1 Branch Hierarchy

```
  main      ──────────●─────────────────────●────── (v0.1.2 Release)
                     /                      │
  develop   ────────●───────●───────────────●────── (Integration)
                   /       /               /
  feature/* ──────●───────/               /        (Topic Feature)
                         /               /
  fix/*     ────────────●───────────────/          (Bug Fix)
```

---

## 8.2 Branch Definitions

### 1. `main` (Production / Stable Releases)
- **Role**: Contains production-ready release code.
- **Protection**: Direct commits to `main` are prohibited. Changes enter `main` strictly via Pull Requests from `develop` or `hotfix/*`.
- **Tags**: Version releases (`v0.1.0`, `v0.1.1`, `v0.1.2`, `v0.2.0`) are tagged directly on `main`.

### 2. `develop` (Integration / Active Development)
- **Role**: Primary integration branch for upcoming releases.
- **Merge Target**: All non-critical feature PRs (`feature/*`), enhancements, and refactors target `develop`.

### 3. Topic Branches (`feature/*`, `fix/*`, `docs/*`, `chore/*`)
- Short-lived developer branches branched off `develop`.
- **Naming Conventions**:
  - `feature/audio-resampling`
  - `feature/termux-opensl`
  - `fix/ptt-key-release`
  - `docs/wiki-branching-model`
  - `chore/deps-bump`

### 4. `hotfix/*` (Emergency Patch)
- Urgent production bug fixes branched directly off `main`.
- Merged into both `main` and `develop`.

---

## 8.3 Pull Request & Release Workflow

1. **Branch Out**: Create a branch off `develop`:
   ```bash
   git checkout develop
   git pull origin develop
   git checkout -b feature/my-new-feature
   ```
2. **Commit & Test**: Ensure code compiles cleanly and passes all 94 unit/integration tests:
   ```bash
   cargo test
   cargo clippy --all-targets
   ```
3. **Open Pull Request**: Target `develop` as the base branch.
4. **CI Verification**: GitHub Actions automatically runs build & test suites on the PR.
5. **Merge**: Once approved, squashed or rebased into `develop`.
6. **Release Cut**: When preparing a release, `develop` is merged into `main`, tagged (e.g. `v0.2.0`), which automatically triggers GitHub Actions binary release builds!

