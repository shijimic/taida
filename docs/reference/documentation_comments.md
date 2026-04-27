# ドキュメントコメント仕様

## 概要

ドキュメントコメントは、コードの説明や API ドキュメント生成、AI/LLM との協業を支援するための構造化されたコメントです。

## 設計原則

- **AI協業ファースト**: AI/LLM が理解しやすい構造化メタデータを提供
- **シンプルな構文**: `///@` プレフィックスで統一
- **豊富なメタデータ**: 目的、例、制約など多角的な情報を記述可能
- **ドキュメント生成対応**: 自動的に API ドキュメントを生成可能

---

## 基本構文

### 単一行コメント

```taida
///@ Purpose: この関数の目的を簡潔に説明
```

### 複数行コメント

```taida
///@
///@ 複数行にわたる説明を
///@ 記述できます。
///@
```

### タグ形式

タグは `@タグ名:` の形式で始まり、値が続きます。

```taida
///@ Purpose: パイロットを作成する
///@ Returns: 作成されたパイロットオブジェクト
```

---

## 標準タグ

### @Purpose

関数・型の目的を簡潔に説明します。

```taida
///@ Purpose: 指定されたIDのパイロットを取得する
getPilot id: Int =
  // 実装
=> :Pilot
```

### @Params

パラメータを説明します。

```taida
///@ Params:
///@   - id: パイロットの一意識別子
///@   - includeDeleted: 削除済みパイロットも含めるか
getPilot id: Int includeDeleted: Bool =
  // 実装
=> :Pilot
```

### @Returns

戻り値を説明します。

```taida
///@ Returns: 見つかったパイロット、見つからない場合はデフォルト値
```

### @Throws

発生しうるエラーを説明します。

```taida
///@ Throws:
///@   - ValidationError: IDが負の場合
///@   - NetworkError: サーバーに接続できない場合
```

### @Example

使用例を示します。

```taida
///@ Example:
///@   pilot <= getPilot(123)
///@   stdout(pilot.name)  // "Misato"
```

### @Since

導入バージョンを示します。

```taida
///@ Since: 1.2.0
```

### @Deprecated

非推奨であることを示します。

```taida
///@ Deprecated: 代わりに getPilotById を使用してください
```

### @See

関連する関数・型への参照を示します。

```taida
///@ See: updatePilot, deletePilot
```

---

## AI協業タグ

AI/LLM がコードを理解し、適切に利用するための特別なタグです。

### @AI-Context

AI がこの関数を使用すべき状況を説明します。

```taida
///@ AI-Context:
///@   この関数はパイロット登録フローで使用される。
///@   フォーム入力の検証後、データベースにパイロットを保存する前に呼び出す。
///@   認証が必要なコンテキストでは使用しない。
createPilot name: Str email: Str =
  // 実装
=> :Pilot
```

### @AI-Examples

AI が学習するための複数の使用例を提供します。

```taida
///@ AI-Examples:
///@   // 基本的な使用
///@   pilot <= createPilot("Misato", "asuka@nerv.jp")
///@
///@   // エラーハンドリング付き
///@   result <= createPilot(name, email)
///@     |== error: Error =
///@       stdout("Failed: " + error.message)
///@       Pilot()
///@
///@   // 検証後に使用
///@   | isValidEmail(email) |> createPilot(name, email)
///@   | _ |> Pilot()
```

### @AI-Constraints

AI がこの関数で行ってはいけないことを説明します。

```taida
///@ AI-Constraints:
///@   - 未検証の入力を直接渡さない
///@   - 空文字列のnameを渡さない
///@   - この関数を認証なしで公開APIから呼び出さない
///@   - 戻り値をnullチェックなしで使用しない
```

### @AI-Related

関連する関数・型を示し、AI が文脈を理解するのを助けます。

```taida
///@ AI-Related:
///@   - updatePilot: パイロット情報の更新
///@   - deletePilot: パイロットの削除
///@   - validateEmail: メールアドレスの検証
///@   - Pilot: パイロット型の定義
```

### @AI-Category

機能のカテゴリを示します。

```taida
///@ AI-Category: pilot-management, crud, database
```

使用可能なカテゴリ例:
- `utility`: ユーティリティ関数
- `validation`: 検証関数
- `transformation`: データ変換
- `crud`: CRUD操作
- `authentication`: 認証関連
- `networking`: ネットワーク操作
- `file-io`: ファイル操作
- `math`: 数学関数
- `string`: 文字列操作
- `collection`: コレクション操作

### @AI-Complexity

処理の計算量や複雑さを示します。

```taida
///@ AI-Complexity:
///@   - Time: O(n log n)
///@   - Space: O(n)
///@   - Network: 1 HTTP request
```

### @AI-SideEffects

副作用を明示します。

```taida
///@ AI-SideEffects:
///@   - Database: Creates new pilot record
///@   - Email: Sends welcome email
///@   - Logging: Logs pilot creation event
```

### @AI-Hint

AI が実装や呼び出し方を選ぶときの補助ヒントを明示します。

```taida
///@ AI-Hint:
///@   - Prefer renderFrame for normal TUI redraw loops
///@   - Pair enter/leave style APIs in cleanup paths
```

---

## 型のドキュメント

### 型定義のドキュメント

```taida
///@ Purpose: アプリケーションパイロットを表す
///@ AI-Context: 認証・認可で使用される基本的なパイロット情報
///@ AI-Related: Session, Permission, Role
Pilot = @(
  ///@ パイロットの一意識別子
  id: Int

  ///@ パイロットの表示名
  name: Str

  ///@ メールアドレス（ログインに使用）
  email: Str

  ///@ アカウントの有効状態
  active: Bool
)
```

### モールディング型のドキュメント

```taida
///@ Purpose: 非同期でデータを取得した結果をラップする
///@ AI-Context: API呼び出しの結果を安全に扱うために使用
///@ AI-Examples:
///@   result <= fetchPilot(id)
///@   | result.success |> result ]=> pilot; stdout(pilot.name)
///@   | _ |> stdout("Error: " + result.error.message)
Mold[T] => ApiResult[T] = @(
  success: Bool
  error: ApiError
  timestamp: Int
)
```

---

## 完全な例

### 関数のフルドキュメント

```taida
///@ Purpose: 指定された条件でパイロットを検索する
///@
///@ Params:
///@   - query: 検索クエリ（名前またはメールの部分一致）
///@   - limit: 最大取得件数（デフォルト: 10）
///@   - includeInactive: 非アクティブパイロットを含めるか
///@
///@ Returns: 条件に一致するパイロットのリスト
///@
///@ Throws:
///@   - ValidationError: queryが空の場合
///@   - DatabaseError: データベース接続エラー
///@
///@ Example:
///@   pilots <= searchPilots("asuka", 5, false)
///@   Map[pilots, _ u = stdout(u.name)]()
///@
///@ AI-Context:
///@   管理画面のパイロット検索機能で使用される。
///@   大量のデータを扱う可能性があるため、limitを適切に設定すること。
///@   パフォーマンスを考慮し、queryは最低3文字以上を推奨。
///@
///@ AI-Examples:
///@   // 基本的な検索
///@   pilots <= searchPilots("rei", 10, false)
///@
///@   // 非アクティブパイロットも含めて検索
///@   allPilots <= searchPilots("test", 100, true)
///@
///@   // エラーハンドリング付き
///@   result <= searchPilots(query, limit, false)
///@     |== error: ValidationError =
///@       @[]
///@
///@ AI-Constraints:
///@   - 空のqueryを渡さない
///@   - limitに極端に大きな値（1000以上）を設定しない
///@   - 認証なしのコンテキストで呼び出さない
///@
///@ AI-Related: getPilot, createPilot, updatePilot, Pilot
///@ AI-Category: pilot-management, search, database
///@ AI-Complexity:
///@   - Time: O(n) where n is the number of pilots
///@   - Space: O(limit)
///@   - Database: 1 query
///@
///@ Since: 1.0.0
searchPilots query: Str limit: Int includeInactive: Bool =
  |== error: Error =
    @[]
  => :@[Pilot]

  | query.length() < 1 |>
    ValidationError(message <= "Query cannot be empty").throw()
  | _ |>
    // データベースクエリの実行
    results <= db.query(
      "SELECT * FROM pilots WHERE name LIKE ? OR email LIKE ?",
      @["%" + query + "%", "%" + query + "%"]
    )
    results ]=> pilots

    filtered <= (
      | includeInactive |> pilots
      | _ |> Filter[pilots, _ u = u.active]() ]=> active; active
    )

    Take[filtered, limit]() ]=> limited
    limited
=> :@[Pilot]
```

---

## ドキュメント生成

ドキュメントコメントから API ドキュメントを自動生成できます。

### 生成コマンド

```bash
taida doc generate ./src --output ./docs/api
```

### 出力形式

- **HTML**: ブラウザで閲覧可能なドキュメント
- **Markdown**: GitHub 等で表示可能な形式
- **JSON**: プログラムから利用可能な形式

### 生成例

```markdown
## searchPilots

指定された条件でパイロットを検索する

### Parameters

| Name | Type | Description |
|------|------|-------------|
| query | Str | 検索クエリ（名前またはメールの部分一致） |
| limit | Int | 最大取得件数（デフォルト: 10） |
| includeInactive | Bool | 非アクティブパイロットを含めるか |

### Returns

`@[Pilot]` - 条件に一致するパイロットのリスト

### Example

```taida
pilots <= searchPilots("asuka", 5, false)
Map[pilots, _ u = stdout(u.name)]()
```

### Related

- `getPilot` - 単一パイロットの取得
- `createPilot` - パイロットの作成
- `updatePilot` - パイロットの更新
```

---

## ベストプラクティス

### 1. 公開APIには必ずドキュメントを付ける

```taida
// 良い例
///@ Purpose: パイロットを作成する
///@ Params:
///@   - name: パイロット名
///@ Returns: 作成されたパイロット
createPilot name: Str =
  // 実装
=> :Pilot

// 悪い例（ドキュメントなし）
createPilot name: Str =
  // 実装
=> :Pilot
```

### 2. AI-Contextは具体的に書く

```taida
// 良い例
///@ AI-Context:
///@   パイロット登録フォームの送信時に呼び出される。
///@   メールアドレスの形式検証は事前に行われている前提。
///@   重複チェックはこの関数内で行われる。

// 悪い例（曖昧）
///@ AI-Context: パイロット作成に使う
```

### 3. 例は実際に動作するコードで書く

```taida
// 良い例
///@ Example:
///@   config <= loadConfig("app.json")
///@   config ]=> cfg
///@   stdout("Port: " + cfg.port.toString())

// 悪い例（擬似コード）
///@ Example:
///@   cfg = loadConfig(path)
///@   print cfg.port
```

### 4. 制約は具体的な値で示す

```taida
// 良い例
///@ AI-Constraints:
///@   - nameは1文字以上100文字以下
///@   - emailは有効なメールアドレス形式
///@   - ageは0以上150以下

// 悪い例（曖昧）
///@ AI-Constraints:
///@   - 適切な値を渡すこと
```

---

## タグ一覧

### 標準タグ

| タグ | 説明 |
|------|------|
| `@Purpose` | 目的の説明 |
| `@Params` | パラメータの説明 |
| `@Returns` | 戻り値の説明 |
| `@Throws` | 発生しうるエラー |
| `@Example` | 使用例 |
| `@Since` | 導入バージョン |
| `@Deprecated` | 非推奨の説明 |
| `@See` | 関連項目 |

### AI協業タグ

| タグ | 説明 |
|------|------|
| `@AI-Context` | 使用すべき状況 |
| `@AI-Examples` | 複数の使用例 |
| `@AI-Constraints` | 制約・禁止事項 |
| `@AI-Related` | 関連する関数・型 |
| `@AI-Category` | 機能カテゴリ |
| `@AI-Complexity` | 計算量・複雑さ |
| `@AI-SideEffects` | 副作用 |
| `@AI-Hint` | 実装・利用時の補助ヒント |
