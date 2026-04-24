# Patch Based Engineering

一套把「一次性的 patch」提升成「可重建的客製化能力」的方法論。

核心目標：

1. 先把 patch 的**意圖**抽出來
2. 再把意圖寫成**可重放的規格**
3. 最後用**乾淨 baseline + subagent 重建**來驗證規格是否真的夠用

---

## When to Use

適合用在這些情境：

- 你手上有一份舊 patch，但上游已經變動，不能直接套用
- 你做的是長期客製化，未來升級上游後需要反覆加回
- 你不想把「怎麼改」綁死在某次 diff，而是想保留「為什麼要這樣改」

---

## Core Artifacts

Patch Based Engineering 建議維護兩份文件：

### `patches.md`

**用途：未來重建用的 source of truth。**

它描述：

- 這組客製化功能是什麼
- 功能邊界在哪裡
- 必須滿足哪些行為
- 如何驗證這組功能是否真的被加回

`patches.md` 是**前向規格**，重點是讓未來的人或 subagent 能重新做出功能。

### `patched.md`

**用途：本次已實作結果的 ledger / delivery report。**

它描述：

- 這次實際改了什麼
- 實際改到哪些檔案
- 跑了哪些驗證
- 還有哪些已知 ambiguity / tech debt

`patched.md` 是**後向紀錄**，重點是保留本次落地的事實與證據。

### Rule of Thumb

- 要讓功能能重做 → 看 `patches.md`
- 要知道這次到底做了什麼 → 看 `patched.md`

---

## How to Produce `patched.md`

`patched.md` 應在功能已經落地、驗證完成後再寫。

建議結構：

```md
# patched - <feature name>

## Goal
這次要把什麼功能加回來

## Inputs
- 來源 patch / commit / issue
- 參考的 `patches.md`

## Changed Files
- file A
- file B

## What Was Restored
- 行為 1
- 行為 2

## Validation
- baseline tests
- post-change tests
- manual scenarios

## Gaps / Ambiguities
- 還有什麼沒寫死
- 哪裡靠工程判斷補齊

## Verdict
- 是否已達到 feature-level restore
```

撰寫原則：

1. `patched.md` 要寫**事實**
2. 不要把它寫成 wish list
3. 不要把 spec 與 implementation report 混在一起
4. 要保留驗證結果，包含失敗但可解釋的項目

---

## How to Write `patches.md`

`patches.md` 要寫成**未來可重建**的文件，而不是本次 diff 導讀。

建議結構如下：

```md
# Custom Patch Spec

## feature-01 - <feature name>

### Scope
### <sub-spec A>
#### Intent
#### Required Behavior
#### Platform-Specific Notes
#### Implementation Anchors
#### Compatibility Rules

### <sub-spec B>
...

### feature-01 Reapply Checklist After Upstream Updates
### feature-01 Test Plan
### feature-01 Current Non-Goals

## Document Practical Goal
```

### What `patches.md` Must Contain

#### 1. Feature-level grouping

一定要以 `feature-xx` 為單位。

原因：

- 未來回補要以完整功能為單位
- 不能只做部分子規格就宣稱功能已恢復

#### 2. Sub-specs that describe responsibility boundaries

每個子章節都要代表一塊清楚的責任，例如：

- inbound handoff
- protocol / capability handling
- outbound roundtrip
- adapter integration contract

#### 3. Intent

每個子規格都要先寫**為什麼存在**，而不是直接寫實作細節。

#### 4. Required Behavior

這是最重要的部分。  
必須寫出：

- 成功時的行為
- 失敗時的 fallback
- 上限 / 條件 / gating
- 不可破壞的既有行為

#### 5. Reapply Checklist

Checklist 應該是**行為導向**，不是檔名導向。

好例子：

- agent 不支援 embedded context 時，仍能收到 file context
- outbound local image 會被重新上傳到 chat

壞例子：

- 一定要有 `src/outbound.rs`

檔名可以放在 `Implementation Anchors`，但 checklist 不應從結果回推未來實作。

#### 6. Test Plan

一定要同時包含：

- automated expectations
- manual verification scenarios
- acceptance rule

#### 7. Non-Goals

要明確寫出這組 feature 不處理什麼，避免未來回補時 scope 膨脹。

#### 8. Wire-level details for ambiguous integrations

如果功能牽涉 protocol / external API，應把容易產生歧義的地方寫死，例如：

- JSON wire shape
- payload 欄位
- fallback policy
- path sanitization rules

否則 subagent 很容易「理解方向正確，但實作細節飄掉」。

---

## Patch Based Engineering Workflow

### Phase 1 - Extract intent from the raw patch

輸入可能是：

- `changes.patch`
- 某個 commit range
- 已存在的客製化分支

工作內容：

1. 列出 patch 影響檔案
2. 找出每個修改背後真正的 feature intent
3. 把 formatting / incidental changes 和核心行為分開
4. 把一堆 scattered edits 收斂成 1~N 個 `feature-xx`

產出：

- feature list
- 每個 feature 的子責任切分

### Phase 2 - Write `patches.md`

把 Phase 1 的 feature intent 寫成規格。

原則：

1. 用 feature grouping
2. 用 behavior language，不要只寫 code diff
3. 把會讓未來重建失敗的模糊點寫死
4. 補上 test plan、reapply checklist、non-goals

### Phase 3 - Implement on current upstream

根據 `patches.md` 在最新 upstream 上移植功能。

這時可以參考舊 patch，但應以 `patches.md` 為準，而不是盲套原始 diff。

### Phase 4 - Produce `patched.md`

當前這次移植完成後，把實際落地情況寫成 `patched.md`。

### Phase 5 - Validate `patches.md`

這是 Patch Based Engineering 最關鍵的一步：

**不要只驗證 code 能跑，還要驗證 `patches.md` 能不能重建功能。**

---

## How to Validate `patches.md`

### Validation Goal

確認這份規格文件是否足以讓一個不看原客製化實作的人 / subagent，仍然把 feature 重建出來。

### Recommended Method

#### 1. Create a clean baseline

建立乾淨 worktree 或 snapshot：

- 必須是沒有本地客製化的 baseline
- 但要把 `patches.md` 放進去

#### 2. Isolate the subagent

對 subagent 下這些限制：

1. 只能在該 baseline worktree 工作
2. 只能把 `patches.md` 當 source of truth
3. 不可讀取主 repo 中已經做好的客製化實作

#### 3. Prefer directed reapply over open-ended exploration

要求它：

- 只根據 `feature-01` 與其子規格
- 只根據 `Implementation Anchors` 決定主要修改面
- 避免無限制廣泛探索

這會更接近真實的「依 spec 回補」能力測試。

#### 4. Review the output

檢查：

- 是否真的把 feature 的核心行為補回
- 是否只改了合理範圍
- 是否產生大量 unrelated churn
- 是否因 spec 模糊而自行發明行為

#### 5. Iterate on ambiguities

如果 subagent：

- 沒做出功能
- 做出來但細節漂掉
- 改太多不相關檔案

就回頭補強 `patches.md`，尤其是：

- wire shape
- payload details
- fallback semantics
- path / naming policy
- feature scope boundaries

### Success Criteria

當以下條件成立時，`patches.md` 可視為達到可重建等級：

1. subagent 在乾淨 baseline 上能回補出 feature 核心行為
2. 不需要偷看原客製化實作
3. 只剩少數 wire-level ambiguity
4. 修改面大致收斂在合理範圍

---

## Anti-Patterns

### 1. Writing `patches.md` as a diff summary

如果只是寫「改了哪些檔案」，未來很難重建。

### 2. Using file names as the source of truth

檔名可能會變，責任邊界才是應該被保留的東西。

### 3. No test plan

沒有 test plan，就只能用「看起來差不多」判斷功能是否回來。

### 4. No subagent retest

如果沒有 spec-only 重建測試，就不知道 `patches.md` 只是可讀，還是真的可執行。

### 5. Mixing `patches.md` and `patched.md`

規格與本次落地報告是兩種不同文件，混在一起會讓兩者都失真。

---

## Practical Template

最小可行流程：

1. 從 raw patch 抽出 `feature-xx`
2. 寫 `patches.md`
3. 在最新 upstream 上移植功能
4. 寫 `patched.md`
5. 建乾淨 worktree
6. 讓 subagent 只靠 `patches.md` 重建
7. review 結果
8. 補強 `patches.md`
9. 重測，直到 spec 足夠穩

---

## Practical Goal

Patch Based Engineering 的最終目標不是保存某一次 patch，  
而是讓一組客製化功能在上游持續演進的情況下，仍然能被**穩定、重複、可驗證地加回來**。
