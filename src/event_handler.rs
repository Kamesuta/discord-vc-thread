use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, Result};
use log::{error, warn};
use serenity::model::{
    application::interaction::{Interaction, InteractionResponseType},
    gateway::Ready,
    guild::Member,
    id::ChannelId,
    prelude::{
        component::{ButtonStyle, InputTextStyle, ActionRowComponent},
        Channel, ChannelType, GuildChannel, interaction::{message_component::MessageComponentInteraction, modal::ModalSubmitInteraction},
    },
    voice::VoiceState,
};

use crate::app_config::AppConfig;

use serenity::async_trait;
use serenity::prelude::*;

/// イベント受信リスナー
pub struct Handler {
    /// 設定
    app_config: AppConfig,
    /// VC→スレッドのマップ
    vc_to_thread: Arc<Mutex<HashMap<ChannelId, ChannelId>>>,
    /// スレッド→VCのマップ
    thread_to_vc: Arc<Mutex<HashMap<ChannelId, ChannelId>>>,
}

impl Handler {
    /// コンストラクタ
    pub fn new(app_config: AppConfig) -> Result<Self> {
        Ok(Self {
            app_config,
            vc_to_thread: Arc::new(Mutex::new(HashMap::new())),
            thread_to_vc: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// カスタムVCかどうか判定する
    fn is_custom_vc(&self, channel: &GuildChannel) -> bool {
        // チャンネルがVCでない場合は無視
        if channel.kind != ChannelType::Voice {
            return false;
        }

        // 親チャンネルID(≒カテゴリID)取得
        let parent_channel_id = match channel.parent_id {
            Some(id) => id,
            None => return false,
        };

        // 親チャンネルIDがカスタムVCカテゴリかどうか判定
        if parent_channel_id != self.app_config.discord.vc_category {
            return false;
        }

        // チャンネルが無視されるチャンネルかどうか判定
        if self
            .app_config
            .discord
            .vc_ignored_channels
            .contains(&channel.id)
        {
            return false;
        }

        true
    }

    /// 参加時にスレッドを作成する
    async fn create_or_mention_thread(
        &self,
        ctx: &Context,
        vc_channel_id: &ChannelId,
        member: &Member,
    ) -> Result<()> {
        // マップからスレッドのチャンネルIDを取得
        let map = self
            .vc_to_thread
            .lock()
            .await
            .get(vc_channel_id)
            .map(|c| c.clone());
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        match map {
            // スレッドが作成済みの場合
            Some(thread_id) => {
                // スレッドのメンバーを取得
                let members = thread_id
                    .get_thread_members(ctx)
                    .await
                    .context("スレッドメンバーの取得に失敗")?;
                // メンバーが存在しない場合
                if !members
                    .iter()
                    .filter_map(|m| m.user_id)
                    .any(|user_id| user_id == member.user.id)
                {
                    // 参加メッセージ
                    thread_id
                        .send_message(ctx, |m| {
                            m.content(format!("{} さんが参加しました。", member.mention()));
                            m
                        })
                        .await
                        .context("参加メッセージの送信に失敗")?;
                }
            }
            // スレッドが作成されていない場合
            None => {
                // チャンネル名を取得
                let channel_name = vc_channel_id
                    .name(&ctx)
                    .await
                    .unwrap_or("不明なチャンネル".to_string());
                // VCカテゴリチャンネルにメッセージを送信
                let thread_channel = self.app_config.discord.thread_channel;
                // メッセージを送信
                let message = thread_channel
                    .send_message(ctx, |m| {
                        m.content(format!(
                            "{} さんが新しいVCを作成しました。\nVCに参加する→ {}",
                            member.mention(),
                            vc_channel_id.mention(),
                        ));
                        m.allowed_mentions(|m| m.empty_users());
                        m
                    })
                    .await
                    .context("作成メッセージの送信に失敗")?;
                // スレッドを作成
                let thread = thread_channel
                    .create_public_thread(ctx, &message, |m| {
                        m.name(&channel_name);
                        m.kind(ChannelType::PublicThread);
                        m
                    })
                    .await
                    .context("スレッドの作成に失敗")?;
                // VCのテキストにチャンネルメンションを追加
                vc_channel_id
                    .send_message(ctx, |m| {
                        m.content(format!("VCチャット→ {}", thread.mention()));
                        m
                    })
                    .await
                    .context("VCチャットの案内メッセージ作成に失敗")?;
                // 参加メッセージ
                thread
                    .send_message(ctx, |m| {
                        m.content(format!("{} `{}`へようこそ。\n興味を引くチャンネル名に変えてみんなを呼び込もう！", member.mention(), &channel_name));
                        m.components(|c| {
                            c.create_action_row(|f| {
                                f.create_button(|b| {
                                    b.label("📝チャンネル名を変える");
                                    b.style(ButtonStyle::Success);
                                    b.custom_id("rename_button");
                                    b
                                });
                                f
                            });
                            c
                        });        
                        m
                    })
                    .await
                    .context("参加メッセージの作成に失敗")?;

                // VCを登録
                self.thread_to_vc
                    .lock()
                    .await
                    .insert(thread.id, vc_channel_id.clone());

                // スレッドを登録
                self.vc_to_thread
                    .lock()
                    .await
                    .insert(vc_channel_id.clone(), thread.id);
            }
        };

        Ok(())
    }

    /// VC削除時にスレッドをアーカイブする
    async fn archive_thread(&self, ctx: &Context, vc_channel_id: &ChannelId) -> Result<()> {
        // マップからスレッドのチャンネルIDを取得
        let channel_id = self
            .vc_to_thread
            .lock()
            .await
            .get(vc_channel_id)
            .map(|c| c.clone());
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        match channel_id {
            // スレッドが作成済みの場合
            Some(thread_id) => {
                // スレッドをアーカイブ
                thread_id
                    .edit_thread(ctx, |t| {
                        t.archived(true);
                        t
                    })
                    .await
                    .context("スレッドのアーカイブに失敗")?;
            }
            // スレッドが作成されていない場合
            None => {}
        };

        Ok(())
    }

    /// VC名前変更時にスレッドをリネームする
    async fn rename_thread(&self, ctx: &Context, vc_channel_id: &ChannelId) -> Result<()> {
        // マップからスレッドのチャンネルIDを取得
        let channel_id = self
            .vc_to_thread
            .lock()
            .await
            .get(vc_channel_id)
            .map(|c| c.clone());
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        match channel_id {
            // スレッドが作成済みの場合
            Some(thread_id) => {
                // チャンネル名を取得
                let channel_name = vc_channel_id
                    .name(&ctx)
                    .await
                    .unwrap_or("不明なチャンネル".to_string());
                // スレッドをリネーム
                thread_id
                    .edit_thread(ctx, |t| {
                        t.name(channel_name);
                        t
                    })
                    .await
                    .context("スレッドのリネームに失敗")?;
            }
            // スレッドが作成されていない場合
            None => {}
        };

        Ok(())
    }

    /// VCを取得
    async fn get_vc(&self, ctx: &Context, channel_id: &ChannelId) -> Result<GuildChannel> {
        // マップからスレッドのチャンネルIDを取得
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        let vc_channel_id = self.thread_to_vc.lock().await.get(channel_id).map(|c| c.clone()).ok_or(anyhow::anyhow!("無効なVCチャンネル"))?;
        let vc_channel = vc_channel_id.to_channel(&ctx).await.context("チャンネルの取得に失敗")?;
        let vc_channel = vc_channel.guild().ok_or(anyhow::anyhow!("無効なVCチャンネルの種類"))?;
        Ok(vc_channel)
    }

    /// VC名前変更時にスレッドをリネームする
    async fn button_pressed(&self, ctx: &Context, interaction: &MessageComponentInteraction) -> Result<()> {
        // VCチャンネルを取得
        let vc_channel = match self.get_vc(ctx, &interaction.channel_id).await {
            Ok(vc_channel) => vc_channel,
            Err(_) => return {
                interaction.create_interaction_response(&ctx, |r| {
                    r.kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|d| {
                            d.content("❌そのVCは既に解散しています");
                            d.ephemeral(true);
                            d
                        });
                    r
                })
                .await
                .context("エラー内容の応答に失敗")?;

                Ok(())
            },
        };

        // VCの権限をチェック
        match vc_channel.permissions_for_user(&ctx, interaction.user.id).context("VCチャンネルのパーミッション取得に失敗")? {
            vc_permission if vc_permission.manage_channels() => {},
            _ => return {
                interaction.create_interaction_response(&ctx, |r| {
                    r.kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|d| {
                            d.content("❌VCのオーナーのみが名前を変更できます");
                            d.ephemeral(true);
                            d
                        });
                    r
                })
                .await
                .context("エラー内容の応答に失敗")?;

                Ok(())
            },
        };

        // モーダルダイアログを開く
        interaction.create_interaction_response(&ctx, |r| {
            r.kind(InteractionResponseType::Modal)
                .interaction_response_data(|d| {
                    d.custom_id("rename_title");
                    d.title("✏️チャンネル名を変える");
                    d.components(|c| {
                        c.create_action_row(|f| {
                            f.create_input_text(|t| {
                                t.custom_id("rename_text");
                                t.label("VCのテーマは？");
                                t.placeholder("フォートナイト, しりとり, カラオケ,...");
                                t.style(InputTextStyle::Short);
                                t
                            });
                            f
                        });
                        c
                    });
                    d
                });
            r
        })
        .await
        .context("ダイアログの作成に失敗")?;

        Ok(())
    }

    /// VC名前変更時にスレッドをリネームする
    async fn rename_vc(&self, ctx: &Context, interaction: &ModalSubmitInteraction) -> Result<()> {
        // VCチャンネルを取得
        let mut vc_channel = match self.get_vc(ctx, &interaction.channel_id).await {
            Ok(vc_channel) => vc_channel,
            Err(_) => return {
                interaction.create_interaction_response(&ctx, |r| {
                    r.kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|d| {
                            d.content("❌そのVCは既に解散しています");
                            d.ephemeral(true);
                            d
                        });
                    r
                })
                .await
                .context("エラー内容の応答に失敗")?;

                Ok(())
            },
        };

        // VCの権限をチェック
        match vc_channel.permissions_for_user(&ctx, interaction.user.id).context("VCチャンネルのパーミッション取得に失敗")? {
            vc_permission if vc_permission.manage_channels() => {},
            _ => return {
                interaction.create_interaction_response(&ctx, |r| {
                    r.kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|d| {
                            d.content("❌VCのオーナーのみが名前を変更できます");
                            d.ephemeral(true);
                            d
                        });
                    r
                })
                .await
                .context("エラー内容の応答に失敗")?;

                Ok(())
            },
        };

        // VC名前を変更
        let name = interaction.data.components
            .iter()
            .flat_map(|c| c.components.iter())
            .find_map(|c| {
                match c {
                    ActionRowComponent::InputText(t) if t.custom_id == "rename_text" => Some(t.value.clone()),
                    _ => None,
                }
            })
            .ok_or(anyhow::anyhow!("コンポーネントが見つかりません"))?;
        vc_channel.edit(&ctx, |e| {
            e.name(name);
            e
        }).await.context("VC名前変更に失敗")?;

        // 返答
        interaction.create_interaction_response(&ctx, |r| {
            r.kind(InteractionResponseType::ChannelMessageWithSource)
                .interaction_response_data(|d| {
                    d.content("✅名前を変更しました");
                    d.ephemeral(true);
                    d
                });
            r
        })
        .await
        .context("結果の応答に失敗")?;

        Ok(())
    }
}

#[async_trait]
impl EventHandler for Handler {
    /// 準備完了時に呼ばれる
    async fn ready(&self, _ctx: Context, data_about_bot: Ready) {
        warn!("Bot準備完了: {}", data_about_bot.user.tag());
    }

    /// VCで話すボタンが押された時
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        // 不明なインタラクションは無視
        match interaction {
            Interaction::MessageComponent(interaction) if interaction.data.custom_id == "rename_button" => {
                // 名前変更チェック&反応
                match self.button_pressed(&ctx, &interaction).await {
                    Ok(_) => {}
                    Err(why) => {
                        error!("インタラクションの処理に失敗: {:?}", why);
                        return;
                    }
                }
            },
            Interaction::ModalSubmit(interaction) if interaction.data.custom_id == "rename_title" => {
                // テキスト入力があったらVC名前変更
                match self.rename_vc(&ctx, &interaction).await {
                    Ok(_) => {}
                    Err(why) => {
                        error!("インタラクションの処理に失敗: {:?}", why);
                        return;
                    }
                }
            }
            _ => return,
        };
    }

    /// VC削除時
    async fn channel_delete(&self, ctx: Context, channel: &GuildChannel) {
        // カスタムVCでない場合は無視
        if !self.is_custom_vc(channel) {
            return;
        }

        // VCスレッドチャンネルをアーカイブ
        match self.archive_thread(&ctx, &channel.id).await {
            Ok(_) => {}
            Err(why) => {
                error!("VCスレッドチャンネルのアーカイブに失敗: {:?}", why);
                return;
            }
        }
    }

    /// VC名更新時
    async fn channel_update(&self, _ctx: Context, _old: Option<Channel>, new: Channel) {
        // チャンネルを取得
        let vc_channel = match new.guild() {
            Some(guild) => guild,
            None => return,
        };

        // カスタムVCでない場合は無視
        if !self.is_custom_vc(&vc_channel) {
            return;
        }

        // VCスレッドチャンネルをリネーム
        match self.rename_thread(&_ctx, &vc_channel.id).await {
            Ok(_) => {}
            Err(why) => {
                error!("VCスレッドチャンネルのリネームに失敗: {:?}", why);
                return;
            }
        }
    }

    /// VCに参加/退出した時
    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        // チャンネルID、ユーザーが存在しない場合は無視
        if let (Some(vc_channel_id), Some(member)) = (new.channel_id, new.member) {
            // チャンネルを取得
            let vc_channel = match vc_channel_id
                .to_channel(&ctx)
                .await
                .context("チャンネル取得失敗")
                .and_then(|c| c.guild().ok_or(anyhow::anyhow!("チャンネルが存在しません")))
            {
                Ok(channel) => channel,
                Err(why) => {
                    error!("チャンネルの取得に失敗: {:?}", why);
                    return;
                }
            };

            // カスタムVCでない場合は無視
            if !self.is_custom_vc(&vc_channel) {
                return;
            }

            // VCスレッドチャンネルを作成
            match self
                .create_or_mention_thread(&ctx, &vc_channel_id, &member)
                .await
            {
                Ok(_) => {}
                Err(why) => {
                    error!("VCスレッドチャンネルの作成/投稿に失敗: {:?}", why);
                    return;
                }
            }
        }
    }
}
