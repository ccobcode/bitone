use super::protocol::{self, transaction::contract::ContractType};
use prost::Message;
fn again<M: Message + Default>(value: &[u8]) -> Result<Vec<u8>, prost::DecodeError> {
    M::decode(value).map(|message| message.encode_to_vec())
}
pub fn recode(
    kind: ContractType,
    value: &[u8],
) -> Option<Result<Vec<u8>, prost::DecodeError>> {
    match kind {
        ContractType::AccountCreateContract => {
            Some(again::<protocol::AccountCreateContract>(value))
        }
        ContractType::TransferContract => {
            Some(again::<protocol::TransferContract>(value))
        }
        ContractType::TransferAssetContract => {
            Some(again::<protocol::TransferAssetContract>(value))
        }
        ContractType::VoteAssetContract => {
            Some(again::<protocol::VoteAssetContract>(value))
        }
        ContractType::VoteWitnessContract => {
            Some(again::<protocol::VoteWitnessContract>(value))
        }
        ContractType::WitnessCreateContract => {
            Some(again::<protocol::WitnessCreateContract>(value))
        }
        ContractType::AssetIssueContract => {
            Some(again::<protocol::AssetIssueContract>(value))
        }
        ContractType::WitnessUpdateContract => {
            Some(again::<protocol::WitnessUpdateContract>(value))
        }
        ContractType::ParticipateAssetIssueContract => {
            Some(again::<protocol::ParticipateAssetIssueContract>(value))
        }
        ContractType::AccountUpdateContract => {
            Some(again::<protocol::AccountUpdateContract>(value))
        }
        ContractType::FreezeBalanceContract => {
            Some(again::<protocol::FreezeBalanceContract>(value))
        }
        ContractType::UnfreezeBalanceContract => {
            Some(again::<protocol::UnfreezeBalanceContract>(value))
        }
        ContractType::WithdrawBalanceContract => {
            Some(again::<protocol::WithdrawBalanceContract>(value))
        }
        ContractType::UnfreezeAssetContract => {
            Some(again::<protocol::UnfreezeAssetContract>(value))
        }
        ContractType::UpdateAssetContract => {
            Some(again::<protocol::UpdateAssetContract>(value))
        }
        ContractType::ProposalCreateContract => {
            Some(again::<protocol::ProposalCreateContract>(value))
        }
        ContractType::ProposalApproveContract => {
            Some(again::<protocol::ProposalApproveContract>(value))
        }
        ContractType::ProposalDeleteContract => {
            Some(again::<protocol::ProposalDeleteContract>(value))
        }
        ContractType::SetAccountIdContract => {
            Some(again::<protocol::SetAccountIdContract>(value))
        }
        ContractType::CustomContract => None,
        ContractType::CreateSmartContract => {
            Some(again::<protocol::CreateSmartContract>(value))
        }
        ContractType::TriggerSmartContract => {
            Some(again::<protocol::TriggerSmartContract>(value))
        }
        ContractType::GetContract => None,
        ContractType::UpdateSettingContract => {
            Some(again::<protocol::UpdateSettingContract>(value))
        }
        ContractType::ExchangeCreateContract => {
            Some(again::<protocol::ExchangeCreateContract>(value))
        }
        ContractType::ExchangeInjectContract => {
            Some(again::<protocol::ExchangeInjectContract>(value))
        }
        ContractType::ExchangeWithdrawContract => {
            Some(again::<protocol::ExchangeWithdrawContract>(value))
        }
        ContractType::ExchangeTransactionContract => {
            Some(again::<protocol::ExchangeTransactionContract>(value))
        }
        ContractType::UpdateEnergyLimitContract => {
            Some(again::<protocol::UpdateEnergyLimitContract>(value))
        }
        ContractType::AccountPermissionUpdateContract => {
            Some(again::<protocol::AccountPermissionUpdateContract>(value))
        }
        ContractType::ClearAbiContract => {
            Some(again::<protocol::ClearAbiContract>(value))
        }
        ContractType::UpdateBrokerageContract => {
            Some(again::<protocol::UpdateBrokerageContract>(value))
        }
        ContractType::ShieldedTransferContract => {
            Some(again::<protocol::ShieldedTransferContract>(value))
        }
        ContractType::MarketSellAssetContract => {
            Some(again::<protocol::MarketSellAssetContract>(value))
        }
        ContractType::MarketCancelOrderContract => {
            Some(again::<protocol::MarketCancelOrderContract>(value))
        }
        ContractType::FreezeBalanceV2Contract => {
            Some(again::<protocol::FreezeBalanceV2Contract>(value))
        }
        ContractType::UnfreezeBalanceV2Contract => {
            Some(again::<protocol::UnfreezeBalanceV2Contract>(value))
        }
        ContractType::WithdrawExpireUnfreezeContract => {
            Some(again::<protocol::WithdrawExpireUnfreezeContract>(value))
        }
        ContractType::DelegateResourceContract => {
            Some(again::<protocol::DelegateResourceContract>(value))
        }
        ContractType::UnDelegateResourceContract => {
            Some(again::<protocol::UnDelegateResourceContract>(value))
        }
        ContractType::CancelAllUnfreezeV2Contract => {
            Some(again::<protocol::CancelAllUnfreezeV2Contract>(value))
        }
    }
}
