# DiscordカスタムVCスレッドBot

一時的なVC用の聞き専チャットを自動生成します。

[AstroBot](https://astro-bot.space/) や MEE6 Temporary Channels などの一時VC作成Botと併用して使用されることを想定しています。

## 機能

- VCが作成されると設定したテキストチャンネル内に、VCと同名のスレッドチャンネルを作成しメンションを飛ばします。
- VCが削除されるとスレッドチャンネルをアーカイブし、通話時間や参加者などを表示します
- スレッドチャンネル内の「チャンネル名を変える」ボタンを押すことでVCの名前を変えることができます

## 使用想定

- VC作成チャンネルに入ると AstroBot によってVCが作成されます
- VCが作成されると、スレッドチャンネルが作成されます
- スレッドチャンネル内の「チャンネル名を変える」ボタンでVCの名前を変えます
- VCから全員退出すると AstroBot によってVCが削除されます
- VCが削除されると、通話時間や参加者などを表示し、スレッドチャンネルがアーカイブされます

# スクリーンショット

![イメージ](https://user-images.githubusercontent.com/16362824/187069176-f1441f17-03de-4a06-b016-f3bc15465b6e.png)

## セットアップ

- 環境変数 `DISCORD_TOKEN` にBotのトークンを登録します
- `config.default.toml` をコピーし `config.toml` を作成します
- `config.toml` の設定を変更します
- `cargo run` で起動します

|設定名|説明|
|----|----|
|vc_category|一時VCが作成されるカテゴリID|
|vc_ignored_channels|VC作成チャンネルや、参加した際に無視したいチャンネルを指定する|
|thread_channel|スレッドを作成するチャンネル|
