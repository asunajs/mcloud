use super::*;
use crate::app_client::AppProfile;
use mcloud_core::Error;
use serde_json::Value;

// ─────────────────────────────────────────────────────────────────────────────
// 发送
// ─────────────────────────────────────────────────────────────────────────────

impl<C: mcloud_core::HttpClient> crate::Upstream<'_, C> {
    /// 换 SSO token（`querySpecTokenV2`）。
    pub async fn app_query_spec_token(
        &self,
        profile: &AppProfile,
        timestamp: i64,
        authorization: &str,
        to_source_id: &str,
    ) -> Result<String, Error> {
        let body = query_spec_token_body(to_source_id);
        let envelope = self
            .call(query_spec_token_request(
                profile,
                timestamp,
                authorization,
                &body,
            )?)
            .await?;
        parse_sso_token(&envelope)
    }

    /// 取服务端毫秒时间戳（签名要用）。
    pub async fn app_h5_timestamp(&self) -> Result<String, Error> {
        let envelope = self.call(h5_timestamp_request()).await?;
        parse_server_timestamp(&envelope)
    }

    /// 建会话（`tyrzLogin`，带签名）。
    pub async fn app_h5_login(
        &self,
        sign: &H5Signature,
        referer: &str,
        sso_token: &str,
    ) -> Result<H5Login, Error> {
        let body = tyrz_login_body(sso_token);
        let envelope = self.call(tyrz_login_request(sign, referer, &body)?).await?;
        parse_jwt_token(&envelope)
    }

    /// 查签到状态（`infoV3`）。
    pub async fn app_sign_in_status(
        &self,
        jwt: &str,
        referer: &str,
    ) -> Result<SignInStatus, Error> {
        let envelope = self.call(info_v3_request(jwt, referer)).await?;
        parse_sign_in_status(&envelope)
    }

    /// `infoV3` 的**原始响应体**，不做任何解析。
    ///
    /// # 为什么留一个不解析的入口
    ///
    /// [`SignInStatus`] 只保留我们**认识的**字段。协议还在摸（`receiveList[]`
    /// 的条目里到底还有哪些字段、顺序有没有含义），"只看得见的字段"会让排查
    /// 变成对着残缺数据猜 —— 这个入口就是为了那种时候。
    ///
    /// ⚠ 它不是给业务用的：调用方拿到的是上游原文，**字段随上游变**，
    /// 别把它接进任何判断里。
    pub async fn app_sign_in_status_raw(&self, jwt: &str, referer: &str) -> Result<Value, Error> {
        let envelope = self.call(info_v3_request(jwt, referer)).await?;
        Ok(envelope.raw().clone())
    }

    /// 执行签到（`startSignIn`）。
    pub async fn app_start_sign_in(
        &self,
        jwt: &str,
        referer: &str,
        device_id: Option<&str>,
    ) -> Result<SignInResult, Error> {
        let envelope = self
            .call(start_sign_in_request(jwt, referer, device_id))
            .await?;
        parse_start_sign_in(&envelope)
    }

    /// 任务列表（`taskListV3`）。
    pub async fn app_task_list_v3(
        &self,
        jwt: &str,
        referer: &str,
        client: u8,
    ) -> Result<Vec<Value>, Error> {
        let envelope = self
            .call(task_list_v3_request(jwt, referer, client)?)
            .await?;
        parse_task_list(&envelope)
    }

    /// 领云朵（`receiveV3`）。
    ///
    /// `device_id` 是必填的 —— 见 [`receive_v3_request`] 的说明。
    pub async fn app_receive_v3(
        &self,
        jwt: &str,
        referer: &str,
        cloud_id: &str,
        cloud_type: Option<i64>,
        device_id: &str,
    ) -> Result<ReceiveResult, Error> {
        let envelope = self
            .call(receive_v3_request(
                jwt, referer, cloud_id, cloud_type, device_id,
            )?)
            .await?;
        parse_receive_v3(&envelope)
    }

    /// 查豆的收支流水（`cloudRecordV2`）。**只读**，用于对账 / 排障。
    ///
    /// `ai_dou` 取 [`RECORD_BOOK_CLOUD`] / [`RECORD_BOOK_BEAN`]，
    /// `filter` 取 [`RECORD_FILTER_ALL`] / [`RECORD_FILTER_EARNED`] / [`RECORD_FILTER_SPENT`]。
    pub async fn app_cloud_records(
        &self,
        jwt: &str,
        referer: &str,
        ai_dou: i64,
        filter: i64,
        page_number: i64,
        page_size: i64,
    ) -> Result<CloudRecords, Error> {
        let envelope = self
            .call(cloud_record_v2_request(
                jwt,
                referer,
                ai_dou,
                filter,
                page_number,
                page_size,
            ))
            .await?;
        parse_cloud_records(&envelope)
    }

    /// 用复活卡 / 回馈卡（`receiveRevivalReward`）。
    ///
    /// `device_id` 可选 —— 两个证据不一致，见 [`receive_revival_reward_request`]。
    pub async fn app_receive_revival_reward(
        &self,
        jwt: &str,
        referer: &str,
        device_id: Option<&str>,
    ) -> Result<RevivalReward, Error> {
        let envelope = self
            .call(receive_revival_reward_request(jwt, referer, device_id)?)
            .await?;
        parse_revival_reward(&envelope)
    }

    /// 延期领取（`receiveTaskExpansion`）—— `cloudType == 1` 那一条路。
    ///
    /// `accept_date` 取自 `receiveList[]` 里**同一条**的 `acceptDate`。
    pub async fn app_receive_task_expansion(
        &self,
        jwt: &str,
        referer: &str,
        accept_date: Option<&str>,
    ) -> Result<ReceiveTaskExpansion, Error> {
        let envelope = self
            .call(receive_task_expansion_request(jwt, referer, accept_date))
            .await?;
        parse_receive_task_expansion(&envelope)
    }

    /// 领**签到翻倍**（`multiple`）。**写请求**，每月只有一次机会。
    ///
    /// ⚠ 门槛**不在这里判** —— 见 [`multiple_request`] 的文档。调用方要先看
    /// `infoV3` 的 `curMonthBackup` / `curMonthBackupSignAccept`
    /// （可执行版本：`mcloud_app::signin::plan_multiple`）。
    pub async fn app_claim_multiple(
        &self,
        jwt: &str,
        referer: &str,
    ) -> Result<MultipleResult, Error> {
        let envelope = self.call(multiple_request(jwt, referer)).await?;
        parse_multiple(&envelope)
    }

    /// 查**奖品记录**（`record`）。只读。
    pub async fn app_prize_records(
        &self,
        jwt: &str,
        referer: &str,
        marketname: &str,
        status: i64,
        page_number: i64,
        page_size: i64,
    ) -> Result<PrizeRecords, Error> {
        let envelope = self
            .call(prize_record_request(
                jwt,
                referer,
                marketname,
                status,
                page_number,
                page_size,
            ))
            .await?;
        parse_prize_records(&envelope)
    }

    /// 查**备份好礼**状态（`backupgift/info`）。只读。
    pub async fn app_backup_gift(&self, jwt: &str, referer: &str) -> Result<BackupGift, Error> {
        let envelope = self.call(backup_gift_info_request(jwt, referer)).await?;
        parse_backup_gift(&envelope)
    }

    /// 领**备份好礼**（`backupgift/receive`）。**写请求**，每月一次。
    ///
    /// ⚠ 门槛**不在这里判** —— 调用方要先看 [`Self::app_backup_gift`] 的
    /// [`BackupGift::state`] 与 [`BackupGift::cur_month`]。
    /// 可执行版本：`mcloud_app::signin::plan_backup_gift`。
    pub async fn app_receive_backup_gift(
        &self,
        jwt: &str,
        referer: &str,
    ) -> Result<BackupGiftReceive, Error> {
        let envelope = self.call(backup_gift_receive_request(jwt, referer)).await?;
        parse_backup_gift_receive(&envelope)
    }
}
