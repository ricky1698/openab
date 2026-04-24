# Custom Patch Spec

這份文件描述目前在 OpenAB 上加上的客製化功能規格。  
目的不是記錄某次 diff，而是清楚定義**我們想保留的行為**，讓未來上游程式碼更新後，可以依照這份文件重新移植。

這份文件採用兩層結構：

- `feature-xx`：代表一組完整的客製化功能
- 子章節：代表該功能底下的規格單元

未來若要把某個客製化功能加回去，應以 `feature-xx` 為單位完整回補，而不是只挑其中一部分。

---

## feature-01 - Attachment Workspace Handoff and Chat Roundtrip

### Scope

這是一組完整客製化功能，目標是讓聊天平台附件能進入 agent workspace，並讓 agent 產生的本地圖片能回傳到聊天平台。

重新移植 `feature-01` 時，應視為**必須完整包含**以下子規格：

- Inbound Attachment Handoff to Agent Workspace
- ACP Prompt Capability Awareness
- Outbound Local Image Upload Back to Chat
- Adapter Integration Contract

---

### Inbound Attachment Handoff to Agent Workspace

### Intent

當 Discord / Slack 使用者上傳附件時，除了既有的圖片 inline 與音訊 STT 行為外，還要把附件保存到 agent 的 `working_dir`，並把這些檔案以 ACP resource context 傳給 agent，讓 agent 可以直接在工作區中存取使用者上傳的檔案。

### Required Behavior

1. 所有需要落地的附件都必須保存到：

   ```text
   <working_dir>/.openab/attachments/<platform>/<channel>/<message>/
   ```

2. 路徑 segment 與檔名必須做安全化，避免：
   - path traversal
   - 空字串檔名
   - 非檔名安全字元直接進入路徑

   安全化規則應固定如下：
   - 只允許 ASCII 字元中的 `a-z`、`A-Z`、`0-9`、`.`、`-`、`_`
   - 其他字元一律轉成 `_`
   - 結果需去除前後的 `.`
   - 若安全化後為空字串，需使用合理 fallback，例如 `attachment`、`platform`、`channel`、`message`

3. 下載後的附件大小上限為 **25 MB**；超過限制時可略過，但不可中斷整體訊息流程。

4. 對於文字型附件：
   - 若可判定為 text-like 檔案，且大小 **<= 256 KB**
   - 則應優先以 ACP `resource` block 傳送內容
   - 若 agent 不支援 embedded context，必須自動降級為 `resource_link`

5. 對於非文字型或較大的附件：
   - 使用 ACP `resource_link`
   - 仍需指向 workspace 內實際落地的檔案

6. 每個成功落地的附件，都要在 prompt 中額外加入摘要文字，例如：

   ```text
   [Attached files]
   - foo.csv (text/csv) saved to `.openab/attachments/...`
   ```

7. 既有行為必須保留：
   - 若 agent 支援 `promptCapabilities.image`，圖片仍優先走現有 image inline prompt block
   - 若 agent **不支援** `promptCapabilities.image`，圖片不得再以 ACP image block 傳送，而應回退成與一般附件相同的 workspace persistence + `resource` / `resource_link` 流程
   - 音訊若啟用 STT，仍要注入 transcript
   - 但音訊與一般檔案也應同時可被持久化為 workspace resource

### Platform-Specific Notes

- **Discord**
  - 使用 attachment URL 下載
  - 在 adapter 組 prompt block 時補上 persisted resource

- **Slack**
  - 私有檔案下載必須帶 Bearer token
  - 在 adapter 組 prompt block 時補上 persisted resource

### Implementation Anchors

- `src/media.rs`
- `src/discord.rs`
- `src/slack.rs`
- `src/adapter.rs`
- `src/acp/pool.rs`

### Compatibility Rules

- 不可破壞原本沒有附件時的訊息流程
- 不可改變現有 thread / session routing
- 不可讓附件功能依賴特定 agent；必須以 ACP capability 做降級

---

### ACP Prompt Capability Awareness

### Intent

不同 ACP agent 對 prompt block 的支援不同。系統必須在 initialize 後讀取 agent capability，決定是否可以送 `resource`、是否只能送 `resource_link`，避免把不支援的 prompt 格式硬送出去。

### Required Behavior

1. `initialize` 完成後，需讀取：

   - `agentCapabilities.loadSession`
   - `agentCapabilities.promptCapabilities.image`
   - `agentCapabilities.promptCapabilities.audio`
   - `agentCapabilities.promptCapabilities.embeddedContext`

2. `ContentBlock` 必須支援至少以下型別：

   - `Text`
   - `Image`
   - `ResourceLink`
   - `Resource`

3. `session/prompt` 送出前，必須做 capability-aware 轉換：

   - `Resource` + `embeddedContext = true` → 送 `resource`
   - `Resource` + `embeddedContext = false` → 自動降級成 `resource_link`
   - `Image` + `image = false` → 不送 ACP image block，而是回退成 workspace attachment handoff

4. 降級必須是**透明且安全**的：
   - 不能因為 agent 不支援 embedded context 就整個丟失附件
   - 最差也要留下可用的 `file://...` link

5. ACP wire shape 應固定如下：

   `resource_link` 範例：

   ```json
   {
     "type": "resource_link",
     "uri": "file:///workspace/.openab/attachments/discord/123/456/example.txt",
     "name": "example.txt",
     "mimeType": "text/plain",
     "size": 123
   }
   ```

   `resource` 範例：

   ```json
   {
     "type": "resource",
     "resource": {
       "uri": "file:///workspace/.openab/attachments/discord/123/456/example.txt",
       "mimeType": "text/plain",
       "text": "hello world"
     }
   }
   ```

   補充規則：
   - `resource` 內不需要再放 `name`
   - 若是文字內容，用 `text`
   - 若未來有二進位嵌入需求，才用 `blob`
   - 若無法合法形成 `resource`，就必須回退成 `resource_link`

### Acceptance Criteria

未支援 embedded context 的 agent，仍能收到 file context；  
支援 embedded context 的 agent，則能直接拿到小型文字附件內容。  
未支援 image prompt capability 的 agent，仍能透過 workspace attachment handoff 取得圖片檔案。

### Implementation Anchors

- `src/acp/connection.rs`

---

### Outbound Local Image Upload Back to Chat

### Intent

當 agent 在回覆中輸出本機工作目錄內的圖片參考，例如：

```md
![plot](out/plot.png)
```

OpenAB 應該把這些圖片真正上傳回 Discord / Slack，而不是把本地路徑原樣顯示給聊天使用者。

### Required Behavior

1. 在最終回覆文字送出前，掃描 Markdown image syntax：

   ```md
   ![alt](path)
   ```

2. 僅接受下列目標：

   - `working_dir` 內的本地檔案
   - 相對路徑、`file://...`，或指向 `working_dir` 內檔案的絕對路徑
   - 副檔名為常見圖片格式：`png/jpg/jpeg/gif/webp`

3. 必須拒絕：

   - `http://`
   - `https://`
   - `data:`
   - `working_dir` 外的路徑
   - 不存在的檔案
   - 非圖片檔
   - 超過 **25 MB** 的檔案

4. 清理輸出文字時：
   - 若圖片成功被辨識為可上傳檔案，原本的 `![alt](path)` 不應直接顯示給使用者
   - 有 alt text 時，用 alt text 取代
   - 沒有 alt text 時，移除該語法

5. 若最終清理後文字為空，但有成功擷取出 outbound image，則主訊息內容應改為：

   ```text
   _(see attached file)_
   ```

6. 相同圖片重複引用時，應去重複，只上傳一次。

### Platform-Specific Notes

- **Discord**
  - 以 adapter 檔案上傳 API 傳回圖片

- **Slack**
  - 使用 external upload flow：
    - 對 `files.getUploadURLExternal` 發送請求，至少帶 `filename` 與 `length`
    - 將檔案 bytes 上傳到回傳的 `upload_url`
    - 對 `files.completeUploadExternal` 發送請求，至少帶：
      - `files: [{ id, title }]`
      - `channel_id`
      - 若在 thread 中則帶 `thread_ts`

  這裡的 payload 語意應固定為：

  `files.getUploadURLExternal`：

  ```text
  filename=<file name>
  length=<byte length>
  ```

  `files.completeUploadExternal`：

  ```json
  {
    "files": [
      {
        "id": "F123",
        "title": "plot.png"
      }
    ],
    "channel_id": "C123",
    "thread_ts": "12345.6789"
  }
  ```

### Implementation Anchors

- `src/outbound.rs`
- `src/adapter.rs`
- `src/discord.rs`
- `src/slack.rs`

---

### Adapter Integration Contract

### Intent

附件 handoff 與 outbound upload 不應散落在各處，必須透過 adapter/router 的共用介面統一整合，讓未來平台行為一致。

### Required Behavior

1. `SessionPool` 必須能提供 `working_dir()` 給 router / adapter 使用。

2. `AdapterRouter` 必須能提供 `working_dir()` 給平台 adapter 使用。

3. `ChatAdapter` 介面需提供：

   - `send_message`
   - `edit_message`
   - `create_thread`
   - `add_reaction`
   - `remove_reaction`
   - `send_attachments`

4. `send_attachments` 的語意是：
   - 將本機檔案路徑作為新訊息上傳到對應聊天平台
   - 不取代主文字訊息
   - 由各平台 adapter 自行實作

---

### feature-01 Reapply Checklist After Upstream Updates

這份 checklist 是 **`feature-01` 專用** 的回補檢查表。  
它描述的是**必須恢復的行為**，不是要求未來一定要沿用這次實作時的檔名、模組切法或檔案結構。

未來重新移植時，至少確認以下行為都成立：

1. 聊天平台上傳的附件會被保存到 agent `working_dir` 之下，且保存路徑符合本文件定義的 attachment 目錄規格。
2. 文字型小附件會在 agent 支援 embedded context 時直接作為 resource 內容提供；不支援時會安全降級為 resource link。
3. 非文字型或較大的附件仍會作為 workspace 中可存取的 file context 提供給 agent。
4. 既有圖片 inline prompt 與音訊 STT 行為仍保留，不會因為附件落地功能而消失。
5. ACP initialize 後會讀取 prompt capability，並據此決定 prompt block 的實際傳送格式。
6. agent 回覆中的本地 Markdown 圖片參考，若指向 `working_dir` 內合法圖片，會被重新上傳到 Discord / Slack，而不是把本地路徑直接顯示給終端使用者。
7. 清理 outbound image 後，主文字訊息仍符合本文件定義的替代文字規則，例如 alt text 替換與 `_(see attached file)_` fallback。
8. 平台整合層仍提供統一的 working_dir 存取與附件上傳能力，使 Discord / Slack 行為保持一致。
9. 與這組功能直接相關的文件說明仍存在，至少要能讓後續維護者理解 inbound attachment handoff 與 outbound image roundtrip 的預期行為。

如果上游更新動到以下責任邊界，就要特別檢查 `feature-01` 是否被破壞：

1. ACP initialize / prompt block encoding
2. 聊天平台附件 ingestion 流程
3. agent 最終回覆的文字清理與附件回傳流程
4. working_dir / workspace file context 的整合方式
5. 平台 adapter 的檔案上傳能力

---

### feature-01 Test Plan

重新移植 `feature-01` 後，至少要完成以下測試，才能判定這組功能真的被加回。

#### Automated Test Expectations

1. 驗證 ACP resource 降級邏輯：
   - agent 不支援 embedded context 時，`Resource` 會降級為 `resource_link`
   - agent 支援 embedded context 時，小型文字內容會保留為 `resource`
   - agent 不支援 image prompt capability 時，圖片會回退成 workspace attachment handoff，而不是送 ACP image block

2. 驗證附件路徑安全化與型別判定：
   - 危險字元會被安全化
   - code / text 類型附件會被辨識為 text-like
   - 非 text 類型附件不會誤判為 embedded text
   - 安全化規則符合本文件定義的字元白名單與 fallback 規則

3. 驗證 outbound image 擷取與清理：
   - `![alt](relative/path.png)` 會被抓出
   - 指向 `working_dir` 內檔案的絕對路徑會被接受
   - `file://...` 可被接受
   - `http://`、`https://`、`data:` 會被拒絕
   - `working_dir` 外路徑會被拒絕
   - 重複引用圖片只保留一份
   - 空 alt text 會正確移除語法

#### Manual Verification Scenarios

1. **Discord - text attachment**
   - 在 Discord 對 bot 發送一個小型文字檔，例如 `notes.md`
   - 確認 agent 能看見附件內容或至少收到可用的 workspace file context
   - 確認檔案被保存到 `working_dir/.openab/attachments/...`

2. **Discord - binary attachment**
   - 發送一個非文字附件，例如 `report.pdf`
   - 確認 agent 不會收到錯誤的文字嵌入，但仍能收到 file context
   - 確認流程不中斷，bot 仍正常回覆

3. **Discord - outbound local image**
   - 讓 agent 回覆 `![plot](some/local/path.png)`，且該圖片位於 `working_dir` 內
   - 確認 Discord 看到的是實際上傳的圖片，不是本地檔案路徑

4. **Slack - private file attachment**
   - 在 Slack 上傳一個文字檔與一個二進位檔
   - 確認 private file 可成功下載並進入 workspace handoff
   - 確認 bot 仍可正常產生回覆

5. **Slack - outbound local image**
   - 讓 agent 輸出指向 `working_dir` 內圖片的 Markdown image
   - 確認 Slack thread 中出現實際上傳圖片

6. **Audio compatibility check**
   - 上傳語音或音訊附件
   - 若 STT 啟用，確認 transcript 仍會進 prompt
   - 同時確認這次 attachment handoff 沒有把既有 STT 流程弄壞

#### Acceptance Rule

只有當 automated tests 與上述 manual verification scenarios 都通過時，才可判定 `feature-01` 已成功回補。

---

### feature-01 Current Non-Goals

以下不是 `feature-01` 這組客製化功能要求的範圍：

- 非圖片類型的 outbound 檔案自動回傳到 chat
- 任意外部 URL 的 re-upload
- 讓 agent 直接接收超大型附件
- 改動既有 reaction / streaming / thread ownership 邏輯

---

## Document Practical Goal

如果未來上游程式碼更新導致這些客製化功能消失，重新移植時應以每個 `feature-xx` 為單位判定是否成功加回。

判定原則如下：

1. 若某個 `feature-xx` 已在文件中定義，則必須完整滿足該 feature 底下的所有子規格。
2. 不應只回補部分子規格就宣稱該 feature 已恢復。
3. 未來新增 `feature-02`、`feature-03` 時，也應沿用同樣結構：每個 feature 都有自己的 scope、reapply checklist、non-goals 與成功判定依據。
