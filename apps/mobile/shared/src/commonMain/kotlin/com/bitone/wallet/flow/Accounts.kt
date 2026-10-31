package com.bitone.wallet.flow

import com.bitone.wallet.platform.AccountAddress

enum class Network {
    Bitcoin,
    Ethereum,
    Bsc,
    Polygon,
    Tron,
    Solana,
}

/** 首页和收款页显示的账户。地址由核心库从公钥算出，按路径对应。 */
data class AccountRow(val network: Network, val name: String, val path: String)

private const val EVM_PATH = "m/44'/60'/0'/0/0"

/** 主账户的显示名。EVM 三条网络共用一个账户，分开列出来，用户才知道地址能收哪条链。 */
val Accounts =
    listOf(
        AccountRow(Network.Bitcoin, "Bitcoin SegWit", "m/84'/0'/0'"),
        AccountRow(Network.Bitcoin, "Bitcoin Taproot", "m/86'/0'/0'"),
        AccountRow(Network.Ethereum, "Ethereum", EVM_PATH),
        AccountRow(Network.Bsc, "BNB Smart Chain", EVM_PATH),
        AccountRow(Network.Polygon, "Polygon", EVM_PATH),
        AccountRow(Network.Tron, "TRON", "m/44'/195'/0'/0/0"),
        AccountRow(Network.Solana, "Solana", "m/44'/501'/0'/0'"),
    )

/** 只列核心库给的账户。导入钱包多出的 Bitcoin 账户没有固定名字，用核心库给的地址类型。 */
fun accountsFor(rows: List<AccountAddress>): List<AccountRow> = rows.flatMap { row ->
    Accounts.filter { it.path == row.path }
        .ifEmpty { listOf(AccountRow(Network.Bitcoin, "Bitcoin ${row.kind}", row.path)) }
}

/** 网络的展示信息。[id] 是协议里的网络标识，只用来取名字和图标，不参与任何判断。 */
class NetworkInfo(val id: String, val name: String, val icon: Network)

private val Networks =
    listOf(
        NetworkInfo("bitcoin", "Bitcoin", Network.Bitcoin),
        NetworkInfo("btc-testnet4", "Testnet4", Network.Bitcoin),
        NetworkInfo("btc-signet", "Signet", Network.Bitcoin),
        NetworkInfo("btc-testnet3", "Testnet3", Network.Bitcoin),
        NetworkInfo("ethereum", "Ethereum", Network.Ethereum),
        NetworkInfo("sepolia", "Sepolia", Network.Ethereum),
        NetworkInfo("hoodi", "Hoodi", Network.Ethereum),
        NetworkInfo("bsc", "BNB Smart Chain", Network.Bsc),
        NetworkInfo("bsc-testnet", "BSC Testnet", Network.Bsc),
        NetworkInfo("polygon", "Polygon", Network.Polygon),
        NetworkInfo("amoy", "Amoy", Network.Polygon),
        NetworkInfo("tron", "TRON", Network.Tron),
        NetworkInfo("tron-nile", "Nile", Network.Tron),
        NetworkInfo("tron-shasta", "Shasta", Network.Tron),
        NetworkInfo("solana", "Solana", Network.Solana),
        NetworkInfo("solana-devnet", "Devnet", Network.Solana),
        NetworkInfo("solana-testnet", "Solana Testnet", Network.Solana),
    )

fun networkInfo(id: String): NetworkInfo? = Networks.firstOrNull { it.id == id }

/** 列表里的短地址，如 0x7a3F…9C21。 */
fun shortAddress(address: String): String {
    val head = if (address.startsWith("0x") || address.startsWith("bc1") || address.startsWith("tb1")) 6 else 4
    return if (address.length <= head + 5) address else address.take(head) + "…" + address.takeLast(4)
}

/** 需要核对的地方显示完整地址，4 个字符一组。 */
fun addressChunks(address: String): List<String> {
    val head = if (address.startsWith("0x")) 6 else 4
    return listOf(address.take(head)) + address.drop(head).chunked(4)
}
