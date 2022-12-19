mod blockchain;
mod psbt;
mod wallet;

use crate::blockchain::{
    Auth, Blockchain, BlockchainConfig, ElectrumConfig, EsploraConfig, RpcConfig, RpcSyncParams,
};
use crate::psbt::PartiallySignedTransaction;
use crate::wallet::{BumpFeeTxBuilder, TxBuilder, Wallet};

use bdk::bitcoin::blockdata::script::Script as BdkScript;
use bdk::bitcoin::secp256k1::Secp256k1;
use bdk::bitcoin::util::bip32::{DerivationPath as BdkDerivationPath, Fingerprint};
use bdk::bitcoin::{Address as BdkAddress, Network, OutPoint as BdkOutPoint, Txid};
use bdk::blockchain::Progress as BdkProgress;
use bdk::database::any::{SledDbConfiguration, SqliteDbConfiguration};
use bdk::descriptor::{DescriptorXKey, ExtendedDescriptor, IntoWalletDescriptor};
use bdk::keys::bip39::{Language, Mnemonic as BdkMnemonic, WordCount};
use bdk::keys::{
    DerivableKey, DescriptorPublicKey as BdkDescriptorPublicKey,
    DescriptorSecretKey as BdkDescriptorSecretKey, ExtendedKey, GeneratableKey, GeneratedKey,
    KeyMap,
};
use bdk::miniscript::BareCtx;
use bdk::template::{
    Bip44, Bip44Public, Bip49, Bip49Public, Bip84, Bip84Public, DescriptorTemplate,
};
use bdk::wallet::AddressIndex as BdkAddressIndex;
use bdk::wallet::AddressInfo as BdkAddressInfo;
use bdk::{Balance as BdkBalance, BlockTime, Error as BdkError, FeeRate, KeychainKind};
use std::convert::From;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

uniffi_macros::include_scaffolding!("bdk");

/// A output script and an amount of satoshis.
pub struct ScriptAmount {
    pub script: Arc<Script>,
    pub amount: u64,
}

/// A derived address and the index it was found at.
pub struct AddressInfo {
    /// Child index of this address
    pub index: u32,
    /// Address
    pub address: String,
}

impl From<BdkAddressInfo> for AddressInfo {
    fn from(x: bdk::wallet::AddressInfo) -> AddressInfo {
        AddressInfo {
            index: x.index,
            address: x.address.to_string(),
        }
    }
}

/// The address index selection strategy to use to derived an address from the wallet's external
/// descriptor.
pub enum AddressIndex {
    /// Return a new address after incrementing the current descriptor index.
    New,
    /// Return the address for the current descriptor index if it has not been used in a received
    /// transaction. Otherwise return a new address as with AddressIndex::New.
    /// Use with caution, if the wallet has not yet detected an address has been used it could
    /// return an already used address. This function is primarily meant for situations where the
    /// caller is untrusted; for example when deriving donation addresses on-demand for a public
    /// web page.
    LastUnused,
}

impl From<AddressIndex> for BdkAddressIndex {
    fn from(x: AddressIndex) -> BdkAddressIndex {
        match x {
            AddressIndex::New => BdkAddressIndex::New,
            AddressIndex::LastUnused => BdkAddressIndex::LastUnused,
        }
    }
}

/// Type that can contain any of the database configurations defined by the library
/// This allows storing a single configuration that can be loaded into an AnyDatabaseConfig
/// instance. Wallets that plan to offer users the ability to switch blockchain backend at runtime
/// will find this particularly useful.
pub enum DatabaseConfig {
    /// Memory database has no config
    Memory,
    /// Simple key-value embedded database based on sled
    Sled { config: SledDbConfiguration },
    /// Sqlite embedded database using rusqlite
    Sqlite { config: SqliteDbConfiguration },
}

/// A wallet transaction
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransactionDetails {
    /// Transaction id.
    pub txid: String,
    /// Received value (sats)
    /// Sum of owned outputs of this transaction.
    pub received: u64,
    /// Sent value (sats)
    /// Sum of owned inputs of this transaction.
    pub sent: u64,
    /// Fee value (sats) if confirmed.
    /// The availability of the fee depends on the backend. It's never None with an Electrum
    /// Server backend, but it could be None with a Bitcoin RPC node without txindex that receive
    /// funds while offline.
    pub fee: Option<u64>,
    /// If the transaction is confirmed, contains height and timestamp of the block containing the
    /// transaction, unconfirmed transaction contains `None`.
    pub confirmation_time: Option<BlockTime>,
}

impl From<&bdk::TransactionDetails> for TransactionDetails {
    fn from(x: &bdk::TransactionDetails) -> TransactionDetails {
        TransactionDetails {
            fee: x.fee,
            txid: x.txid.to_string(),
            received: x.received,
            sent: x.sent,
            confirmation_time: x.confirmation_time.clone(),
        }
    }
}

// struct Blockchain {
//     blockchain_mutex: Mutex<AnyBlockchain>,
// }
//
// impl Blockchain {
//     fn new(blockchain_config: BlockchainConfig) -> Result<Self, BdkError> {
//         let any_blockchain_config = match blockchain_config {
//             BlockchainConfig::Electrum { config } => {
//                 AnyBlockchainConfig::Electrum(ElectrumBlockchainConfig {
//                     retry: config.retry,
//                     socks5: config.socks5,
//                     timeout: config.timeout,
//                     url: config.url,
//                     stop_gap: usize::try_from(config.stop_gap).unwrap(),
//                     validate_domain: config.validate_domain,
//                 })
//             }
//             BlockchainConfig::Esplora { config } => {
//                 AnyBlockchainConfig::Esplora(EsploraBlockchainConfig {
//                     base_url: config.base_url,
//                     proxy: config.proxy,
//                     concurrency: config.concurrency,
//                     stop_gap: usize::try_from(config.stop_gap).unwrap(),
//                     timeout: config.timeout,
//                 })
//             }
//             BlockchainConfig::Rpc { config } => AnyBlockchainConfig::Rpc(BdkRpcConfig {
//                 url: config.url,
//                 auth: config.auth.into(),
//                 network: config.network,
//                 wallet_name: config.wallet_name,
//                 sync_params: config.sync_params.map(|p| p.into()),
//             }),
//         };
//         let blockchain = AnyBlockchain::from_config(&any_blockchain_config)?;
//         Ok(Self {
//             blockchain_mutex: Mutex::new(blockchain),
//         })
//     }
//
//     fn get_blockchain(&self) -> MutexGuard<AnyBlockchain> {
//         self.blockchain_mutex.lock().expect("blockchain")
//     }
//
//     fn broadcast(&self, psbt: &PartiallySignedTransaction) -> Result<(), BdkError> {
//         let tx = psbt.internal.lock().unwrap().clone().extract_tx();
//         self.get_blockchain().broadcast(&tx)
//     }
//
//     fn estimate_fee(&self, target: u64) -> Result<Arc<FeeRate>, BdkError> {
//         let result: Result<FeeRate, bdk::Error> =
//             self.get_blockchain().estimate_fee(target as usize);
//         result.map(Arc::new)
//     }
//
//     fn get_height(&self) -> Result<u32, BdkError> {
//         self.get_blockchain().get_height()
//     }
//
//     fn get_block_hash(&self, height: u32) -> Result<String, BdkError> {
//         self.get_blockchain()
//             .get_block_hash(u64::from(height))
//             .map(|hash| hash.to_string())
//     }
// }

/// A reference to a transaction output.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutPoint {
    /// The referenced transaction's txid.
    txid: String,
    /// The index of the referenced output in its transaction's vout.
    vout: u32,
}

impl From<&OutPoint> for BdkOutPoint {
    fn from(x: &OutPoint) -> BdkOutPoint {
        BdkOutPoint {
            txid: Txid::from_str(&x.txid).unwrap(),
            vout: x.vout,
        }
    }
}

pub struct Balance {
    // All coinbase outputs not yet matured
    pub immature: u64,
    /// Unconfirmed UTXOs generated by a wallet tx
    pub trusted_pending: u64,
    /// Unconfirmed UTXOs received from an external wallet
    pub untrusted_pending: u64,
    /// Confirmed and immediately spendable balance
    pub confirmed: u64,
    /// Get sum of trusted_pending and confirmed coins
    pub spendable: u64,
    /// Get the whole balance visible to the wallet
    pub total: u64,
}

impl From<BdkBalance> for Balance {
    fn from(bdk_balance: BdkBalance) -> Self {
        Balance {
            immature: bdk_balance.immature,
            trusted_pending: bdk_balance.trusted_pending,
            untrusted_pending: bdk_balance.untrusted_pending,
            confirmed: bdk_balance.confirmed,
            spendable: bdk_balance.get_spendable(),
            total: bdk_balance.get_total(),
        }
    }
}

/// A transaction output, which defines new coins to be created from old ones.
pub struct TxOut {
    /// The value of the output, in satoshis.
    value: u64,
    /// The address of the output.
    address: String,
}

pub struct LocalUtxo {
    outpoint: OutPoint,
    txout: TxOut,
    keychain: KeychainKind,
    is_spent: bool,
}

// This trait is used to convert the bdk TxOut type with field `script_pubkey: Script`
// into the bdk-ffi TxOut type which has a field `address: String` instead
trait NetworkLocalUtxo {
    fn from_utxo(x: &bdk::LocalUtxo, network: Network) -> LocalUtxo;
}

impl NetworkLocalUtxo for LocalUtxo {
    fn from_utxo(x: &bdk::LocalUtxo, network: Network) -> LocalUtxo {
        LocalUtxo {
            outpoint: OutPoint {
                txid: x.outpoint.txid.to_string(),
                vout: x.outpoint.vout,
            },
            txout: TxOut {
                value: x.txout.value,
                address: BdkAddress::from_script(&x.txout.script_pubkey, network)
                    .unwrap()
                    .to_string(),
            },
            keychain: x.keychain,
            is_spent: x.is_spent,
        }
    }
}

/// Trait that logs at level INFO every update received (if any).
pub trait Progress: Send + Sync + 'static {
    /// Send a new progress update. The progress value should be in the range 0.0 - 100.0, and the message value is an
    /// optional text message that can be displayed to the user.
    fn update(&self, progress: f32, message: Option<String>);
}

struct ProgressHolder {
    progress: Box<dyn Progress>,
}

impl BdkProgress for ProgressHolder {
    fn update(&self, progress: f32, message: Option<String>) -> Result<(), BdkError> {
        self.progress.update(progress, message);
        Ok(())
    }
}

impl fmt::Debug for ProgressHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgressHolder").finish_non_exhaustive()
    }
}

/// A Bitcoin address.
struct Address {
    address: BdkAddress,
}

impl Address {
    fn new(address: String) -> Result<Self, BdkError> {
        BdkAddress::from_str(address.as_str())
            .map(|a| Address { address: a })
            .map_err(|e| BdkError::Generic(e.to_string()))
    }

    fn script_pubkey(&self) -> Arc<Script> {
        Arc::new(Script {
            script: self.address.script_pubkey(),
        })
    }
}

/// A Bitcoin script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    script: BdkScript,
}

impl Script {
    fn new(raw_output_script: Vec<u8>) -> Self {
        let script: BdkScript = BdkScript::from(raw_output_script);
        Script { script }
    }
}

#[derive(Clone, Debug)]
enum RbfValue {
    Default,
    Value(u32),
}

/// The result after calling the TxBuilder finish() function. Contains unsigned PSBT and
/// transaction details.
pub struct TxBuilderResult {
    pub(crate) psbt: Arc<PartiallySignedTransaction>,
    pub transaction_details: TransactionDetails,
}

/// Mnemonic phrases are a human-readable version of the private keys.
/// Supported number of words are 12, 15, 18, 21 and 24.
struct Mnemonic {
    internal: BdkMnemonic,
}

impl Mnemonic {
    /// Generates Mnemonic with a random entropy
    fn new(word_count: WordCount) -> Self {
        let generated_key: GeneratedKey<_, BareCtx> =
            BdkMnemonic::generate((word_count, Language::English)).unwrap();
        let mnemonic = BdkMnemonic::parse_in(Language::English, generated_key.to_string()).unwrap();
        Mnemonic { internal: mnemonic }
    }

    /// Parse a Mnemonic with given string
    fn from_string(mnemonic: String) -> Result<Self, BdkError> {
        BdkMnemonic::from_str(&mnemonic)
            .map(|m| Mnemonic { internal: m })
            .map_err(|e| BdkError::Generic(e.to_string()))
    }

    /// Create a new Mnemonic in the specified language from the given entropy.
    /// Entropy must be a multiple of 32 bits (4 bytes) and 128-256 bits in length.
    fn from_entropy(entropy: Vec<u8>) -> Result<Self, BdkError> {
        BdkMnemonic::from_entropy(entropy.as_slice())
            .map(|m| Mnemonic { internal: m })
            .map_err(|e| BdkError::Generic(e.to_string()))
    }

    /// Returns Mnemonic as string
    fn as_string(&self) -> String {
        self.internal.to_string()
    }
}

struct DerivationPath {
    derivation_path_mutex: Mutex<BdkDerivationPath>,
}

impl DerivationPath {
    fn new(path: String) -> Result<Self, BdkError> {
        BdkDerivationPath::from_str(&path)
            .map(|x| DerivationPath {
                derivation_path_mutex: Mutex::new(x),
            })
            .map_err(|e| BdkError::Generic(e.to_string()))
    }
}

#[derive(Debug)]
struct DescriptorSecretKey {
    descriptor_secret_key_mutex: Mutex<BdkDescriptorSecretKey>,
}

impl DescriptorSecretKey {
    fn new(network: Network, mnemonic: Arc<Mnemonic>, password: Option<String>) -> Self {
        let mnemonic = mnemonic.internal.clone();
        let xkey: ExtendedKey = (mnemonic, password).into_extended_key().unwrap();
        let descriptor_secret_key = BdkDescriptorSecretKey::XPrv(DescriptorXKey {
            origin: None,
            xkey: xkey.into_xprv(network).unwrap(),
            derivation_path: BdkDerivationPath::master(),
            wildcard: bdk::descriptor::Wildcard::Unhardened,
        });
        Self {
            descriptor_secret_key_mutex: Mutex::new(descriptor_secret_key),
        }
    }

    fn from_string(private_key: String) -> Result<Self, BdkError> {
        let descriptor_secret_key = BdkDescriptorSecretKey::from_str(private_key.as_str())
            .map_err(|e| BdkError::Generic(e.to_string()))?;
        Ok(Self {
            descriptor_secret_key_mutex: Mutex::new(descriptor_secret_key),
        })
    }

    fn derive(&self, path: Arc<DerivationPath>) -> Result<Arc<Self>, BdkError> {
        let secp = Secp256k1::new();
        let descriptor_secret_key = self.descriptor_secret_key_mutex.lock().unwrap();
        let path = path.derivation_path_mutex.lock().unwrap().deref().clone();
        match descriptor_secret_key.deref() {
            BdkDescriptorSecretKey::XPrv(descriptor_x_key) => {
                let derived_xprv = descriptor_x_key.xkey.derive_priv(&secp, &path)?;
                let key_source = match descriptor_x_key.origin.clone() {
                    Some((fingerprint, origin_path)) => (fingerprint, origin_path.extend(path)),
                    None => (descriptor_x_key.xkey.fingerprint(&secp), path),
                };
                let derived_descriptor_secret_key = BdkDescriptorSecretKey::XPrv(DescriptorXKey {
                    origin: Some(key_source),
                    xkey: derived_xprv,
                    derivation_path: BdkDerivationPath::default(),
                    wildcard: descriptor_x_key.wildcard,
                });
                Ok(Arc::new(Self {
                    descriptor_secret_key_mutex: Mutex::new(derived_descriptor_secret_key),
                }))
            }
            BdkDescriptorSecretKey::Single(_) => Err(BdkError::Generic(
                "Cannot derive from a single key".to_string(),
            )),
        }
    }

    fn extend(&self, path: Arc<DerivationPath>) -> Result<Arc<Self>, BdkError> {
        let descriptor_secret_key = self.descriptor_secret_key_mutex.lock().unwrap();
        let path = path.derivation_path_mutex.lock().unwrap().deref().clone();
        match descriptor_secret_key.deref() {
            BdkDescriptorSecretKey::XPrv(descriptor_x_key) => {
                let extended_path = descriptor_x_key.derivation_path.extend(path);
                let extended_descriptor_secret_key = BdkDescriptorSecretKey::XPrv(DescriptorXKey {
                    origin: descriptor_x_key.origin.clone(),
                    xkey: descriptor_x_key.xkey,
                    derivation_path: extended_path,
                    wildcard: descriptor_x_key.wildcard,
                });
                Ok(Arc::new(Self {
                    descriptor_secret_key_mutex: Mutex::new(extended_descriptor_secret_key),
                }))
            }
            BdkDescriptorSecretKey::Single(_) => Err(BdkError::Generic(
                "Cannot extend from a single key".to_string(),
            )),
        }
    }

    fn as_public(&self) -> Arc<DescriptorPublicKey> {
        let secp = Secp256k1::new();
        let descriptor_public_key = self
            .descriptor_secret_key_mutex
            .lock()
            .unwrap()
            .to_public(&secp)
            .unwrap();
        Arc::new(DescriptorPublicKey {
            descriptor_public_key_mutex: Mutex::new(descriptor_public_key),
        })
    }

    /// Get the private key as bytes.
    fn secret_bytes(&self) -> Vec<u8> {
        let descriptor_secret_key = self.descriptor_secret_key_mutex.lock().unwrap();
        let secret_bytes: Vec<u8> = match descriptor_secret_key.deref() {
            BdkDescriptorSecretKey::XPrv(descriptor_x_key) => {
                descriptor_x_key.xkey.private_key.secret_bytes().to_vec()
            }
            BdkDescriptorSecretKey::Single(_) => {
                unreachable!()
            }
        };

        secret_bytes
    }

    fn as_string(&self) -> String {
        self.descriptor_secret_key_mutex.lock().unwrap().to_string()
    }
}

#[derive(Debug)]
struct DescriptorPublicKey {
    descriptor_public_key_mutex: Mutex<BdkDescriptorPublicKey>,
}

impl DescriptorPublicKey {
    fn from_string(public_key: String) -> Result<Self, BdkError> {
        let descriptor_public_key = BdkDescriptorPublicKey::from_str(public_key.as_str())
            .map_err(|e| BdkError::Generic(e.to_string()))?;
        Ok(Self {
            descriptor_public_key_mutex: Mutex::new(descriptor_public_key),
        })
    }

    fn derive(&self, path: Arc<DerivationPath>) -> Result<Arc<Self>, BdkError> {
        let secp = Secp256k1::new();
        let descriptor_public_key = self.descriptor_public_key_mutex.lock().unwrap();
        let path = path.derivation_path_mutex.lock().unwrap().deref().clone();

        match descriptor_public_key.deref() {
            BdkDescriptorPublicKey::XPub(descriptor_x_key) => {
                let derived_xpub = descriptor_x_key.xkey.derive_pub(&secp, &path)?;
                let key_source = match descriptor_x_key.origin.clone() {
                    Some((fingerprint, origin_path)) => (fingerprint, origin_path.extend(path)),
                    None => (descriptor_x_key.xkey.fingerprint(), path),
                };
                let derived_descriptor_public_key = BdkDescriptorPublicKey::XPub(DescriptorXKey {
                    origin: Some(key_source),
                    xkey: derived_xpub,
                    derivation_path: BdkDerivationPath::default(),
                    wildcard: descriptor_x_key.wildcard,
                });
                Ok(Arc::new(Self {
                    descriptor_public_key_mutex: Mutex::new(derived_descriptor_public_key),
                }))
            }
            BdkDescriptorPublicKey::Single(_) => Err(BdkError::Generic(
                "Cannot derive from a single key".to_string(),
            )),
        }
    }

    fn extend(&self, path: Arc<DerivationPath>) -> Result<Arc<Self>, BdkError> {
        let descriptor_public_key = self.descriptor_public_key_mutex.lock().unwrap();
        let path = path.derivation_path_mutex.lock().unwrap().deref().clone();
        match descriptor_public_key.deref() {
            BdkDescriptorPublicKey::XPub(descriptor_x_key) => {
                let extended_path = descriptor_x_key.derivation_path.extend(path);
                let extended_descriptor_public_key = BdkDescriptorPublicKey::XPub(DescriptorXKey {
                    origin: descriptor_x_key.origin.clone(),
                    xkey: descriptor_x_key.xkey,
                    derivation_path: extended_path,
                    wildcard: descriptor_x_key.wildcard,
                });
                Ok(Arc::new(Self {
                    descriptor_public_key_mutex: Mutex::new(extended_descriptor_public_key),
                }))
            }
            BdkDescriptorPublicKey::Single(_) => Err(BdkError::Generic(
                "Cannot extend from a single key".to_string(),
            )),
        }
    }

    fn as_string(&self) -> String {
        self.descriptor_public_key_mutex.lock().unwrap().to_string()
    }
}

#[derive(Debug)]
struct Descriptor {
    pub extended_descriptor: ExtendedDescriptor,
    pub key_map: KeyMap,
}

impl Descriptor {
    fn new(descriptor: String, network: Network) -> Result<Self, BdkError> {
        let secp = Secp256k1::new();
        let (extended_descriptor, key_map) = descriptor.into_wallet_descriptor(&secp, network)?;
        Ok(Self {
            extended_descriptor,
            key_map,
        })
    }

    fn new_bip44(
        secret_key: Arc<DescriptorSecretKey>,
        keychain_kind: KeychainKind,
        network: Network,
    ) -> Self {
        let derivable_key = secret_key.descriptor_secret_key_mutex.lock().unwrap();

        match derivable_key.deref() {
            BdkDescriptorSecretKey::XPrv(descriptor_x_key) => {
                let derivable_key = descriptor_x_key.xkey;
                let (extended_descriptor, key_map, _) =
                    Bip44(derivable_key, keychain_kind).build(network).unwrap();
                Self {
                    extended_descriptor,
                    key_map,
                }
            }
            BdkDescriptorSecretKey::Single(_) => {
                unreachable!()
            }
        }
    }

    fn new_bip44_public(
        public_key: Arc<DescriptorPublicKey>,
        fingerprint: String,
        keychain_kind: KeychainKind,
        network: Network,
    ) -> Self {
        let fingerprint = Fingerprint::from_str(fingerprint.as_str()).unwrap();
        let derivable_key = public_key.descriptor_public_key_mutex.lock().unwrap();

        match derivable_key.deref() {
            BdkDescriptorPublicKey::XPub(descriptor_x_key) => {
                let derivable_key = descriptor_x_key.xkey;
                let (extended_descriptor, key_map, _) =
                    Bip44Public(derivable_key, fingerprint, keychain_kind)
                        .build(network)
                        .unwrap();

                Self {
                    extended_descriptor,
                    key_map,
                }
            }
            BdkDescriptorPublicKey::Single(_) => {
                unreachable!()
            }
        }
    }

    fn new_bip49(
        secret_key: Arc<DescriptorSecretKey>,
        keychain_kind: KeychainKind,
        network: Network,
    ) -> Self {
        let derivable_key = secret_key.descriptor_secret_key_mutex.lock().unwrap();

        match derivable_key.deref() {
            BdkDescriptorSecretKey::XPrv(descriptor_x_key) => {
                let derivable_key = descriptor_x_key.xkey;
                let (extended_descriptor, key_map, _) =
                    Bip49(derivable_key, keychain_kind).build(network).unwrap();
                Self {
                    extended_descriptor,
                    key_map,
                }
            }
            BdkDescriptorSecretKey::Single(_) => {
                unreachable!()
            }
        }
    }

    fn new_bip49_public(
        public_key: Arc<DescriptorPublicKey>,
        fingerprint: String,
        keychain_kind: KeychainKind,
        network: Network,
    ) -> Self {
        let fingerprint = Fingerprint::from_str(fingerprint.as_str()).unwrap();
        let derivable_key = public_key.descriptor_public_key_mutex.lock().unwrap();

        match derivable_key.deref() {
            BdkDescriptorPublicKey::XPub(descriptor_x_key) => {
                let derivable_key = descriptor_x_key.xkey;
                let (extended_descriptor, key_map, _) =
                    Bip49Public(derivable_key, fingerprint, keychain_kind)
                        .build(network)
                        .unwrap();

                Self {
                    extended_descriptor,
                    key_map,
                }
            }
            BdkDescriptorPublicKey::Single(_) => {
                unreachable!()
            }
        }
    }

    fn new_bip84(
        secret_key: Arc<DescriptorSecretKey>,
        keychain_kind: KeychainKind,
        network: Network,
    ) -> Self {
        let derivable_key = secret_key.descriptor_secret_key_mutex.lock().unwrap();

        match derivable_key.deref() {
            BdkDescriptorSecretKey::XPrv(descriptor_x_key) => {
                let derivable_key = descriptor_x_key.xkey;
                let (extended_descriptor, key_map, _) =
                    Bip84(derivable_key, keychain_kind).build(network).unwrap();
                Self {
                    extended_descriptor,
                    key_map,
                }
            }
            BdkDescriptorSecretKey::Single(_) => {
                unreachable!()
            }
        }
    }

    fn new_bip84_public(
        public_key: Arc<DescriptorPublicKey>,
        fingerprint: String,
        keychain_kind: KeychainKind,
        network: Network,
    ) -> Self {
        let fingerprint = Fingerprint::from_str(fingerprint.as_str()).unwrap();
        let derivable_key = public_key.descriptor_public_key_mutex.lock().unwrap();

        match derivable_key.deref() {
            BdkDescriptorPublicKey::XPub(descriptor_x_key) => {
                let derivable_key = descriptor_x_key.xkey;
                let (extended_descriptor, key_map, _) =
                    Bip84Public(derivable_key, fingerprint, keychain_kind)
                        .build(network)
                        .unwrap();

                Self {
                    extended_descriptor,
                    key_map,
                }
            }
            BdkDescriptorPublicKey::Single(_) => {
                unreachable!()
            }
        }
    }

    fn as_string_private(&self) -> String {
        let descriptor = &self.extended_descriptor;
        let key_map = &self.key_map;
        descriptor.to_string_with_secret(key_map)
    }

    fn as_string(&self) -> String {
        self.extended_descriptor.to_string()
    }
}

uniffi::deps::static_assertions::assert_impl_all!(Wallet: Sync, Send);

// The goal of these tests to to ensure `bdk-ffi` intermediate code correctly calls `bdk` APIs.
// These tests should not be used to verify `bdk` behavior that is already tested in the `bdk`
// crate.
#[cfg(test)]
mod test {
    use crate::*;
    use assert_matches::assert_matches;
    use bdk::bitcoin::hashes::hex::ToHex;
    use bdk::bitcoin::Address;
    use bdk::descriptor::DescriptorError::Key;
    use bdk::keys::KeyError::InvalidNetwork;
    use bdk::wallet::get_funded_wallet;
    use std::str::FromStr;
    use std::sync::Mutex;

    #[test]
    fn test_drain_wallet() {
        let test_wpkh = "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
        let (funded_wallet, _, _) = get_funded_wallet(test_wpkh);
        let test_wallet = Wallet {
            wallet_mutex: Mutex::new(funded_wallet),
        };
        let drain_to_address = "tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt".to_string();
        let drain_to_script = crate::Address::new(drain_to_address)
            .unwrap()
            .script_pubkey();
        let tx_builder = TxBuilder::new()
            .drain_wallet()
            .drain_to(drain_to_script.clone());
        assert!(tx_builder.drain_wallet);
        assert_eq!(tx_builder.drain_to, Some(drain_to_script.script.clone()));

        let tx_builder_result = tx_builder.finish(&test_wallet).unwrap();
        let psbt = tx_builder_result.psbt.internal.lock().unwrap().clone();
        let tx_details = tx_builder_result.transaction_details;

        // confirm one input with 50,000 sats
        assert_eq!(psbt.inputs.len(), 1);
        let input_value = psbt
            .inputs
            .get(0)
            .cloned()
            .unwrap()
            .non_witness_utxo
            .unwrap()
            .output
            .get(0)
            .unwrap()
            .value;
        assert_eq!(input_value, 50_000_u64);

        // confirm one output to correct address with all sats - fee
        assert_eq!(psbt.outputs.len(), 1);
        let output_address = Address::from_script(
            &psbt
                .unsigned_tx
                .output
                .get(0)
                .cloned()
                .unwrap()
                .script_pubkey,
            Network::Testnet,
        )
        .unwrap();
        assert_eq!(
            output_address,
            Address::from_str("tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt").unwrap()
        );
        let output_value = psbt.unsigned_tx.output.get(0).cloned().unwrap().value;
        assert_eq!(output_value, 49_890_u64); // input - fee

        assert_eq!(
            tx_details.txid,
            "312f1733badab22dc26b8dcbc83ba5629fb7b493af802e8abe07d865e49629c5"
        );
        assert_eq!(tx_details.received, 0);
        assert_eq!(tx_details.sent, 50000);
        assert!(tx_details.fee.is_some());
        assert_eq!(tx_details.fee.unwrap(), 110);
        assert!(tx_details.confirmation_time.is_none());
    }

    fn get_descriptor_secret_key() -> DescriptorSecretKey {
        let mnemonic = Mnemonic::from_string("chaos fabric time speed sponsor all flat solution wisdom trophy crack object robot pave observe combine where aware bench orient secret primary cable detect".to_string()).unwrap();
        DescriptorSecretKey::new(Network::Testnet, Arc::new(mnemonic), None)
    }

    fn derive_dsk(
        key: &DescriptorSecretKey,
        path: &str,
    ) -> Result<Arc<DescriptorSecretKey>, BdkError> {
        let path = Arc::new(DerivationPath::new(path.to_string()).unwrap());
        key.derive(path)
    }

    fn extend_dsk(
        key: &DescriptorSecretKey,
        path: &str,
    ) -> Result<Arc<DescriptorSecretKey>, BdkError> {
        let path = Arc::new(DerivationPath::new(path.to_string()).unwrap());
        key.extend(path)
    }

    fn derive_dpk(
        key: &DescriptorPublicKey,
        path: &str,
    ) -> Result<Arc<DescriptorPublicKey>, BdkError> {
        let path = Arc::new(DerivationPath::new(path.to_string()).unwrap());
        key.derive(path)
    }

    fn extend_dpk(
        key: &DescriptorPublicKey,
        path: &str,
    ) -> Result<Arc<DescriptorPublicKey>, BdkError> {
        let path = Arc::new(DerivationPath::new(path.to_string()).unwrap());
        key.extend(path)
    }

    #[test]
    fn test_generate_descriptor_secret_key() {
        let master_dsk = get_descriptor_secret_key();
        assert_eq!(master_dsk.as_string(), "tprv8ZgxMBicQKsPdWuqM1t1CDRvQtQuBPyfL6GbhQwtxDKgUAVPbxmj71pRA8raTqLrec5LyTs5TqCxdABcZr77bt2KyWA5bizJHnC4g4ysm4h/*");
        assert_eq!(master_dsk.as_public().as_string(), "tpubD6NzVbkrYhZ4WywdEfYbbd62yuvqLjAZuPsNyvzCNV85JekAEMbKHWSHLF9h3j45SxewXDcLv328B1SEZrxg4iwGfmdt1pDFjZiTkGiFqGa/*");
    }

    #[test]
    fn test_derive_self() {
        let master_dsk = get_descriptor_secret_key();
        let derived_dsk: &DescriptorSecretKey = &derive_dsk(&master_dsk, "m").unwrap();
        assert_eq!(derived_dsk.as_string(), "[d1d04177]tprv8ZgxMBicQKsPdWuqM1t1CDRvQtQuBPyfL6GbhQwtxDKgUAVPbxmj71pRA8raTqLrec5LyTs5TqCxdABcZr77bt2KyWA5bizJHnC4g4ysm4h/*");

        let master_dpk: &DescriptorPublicKey = &master_dsk.as_public();
        let derived_dpk: &DescriptorPublicKey = &derive_dpk(master_dpk, "m").unwrap();
        assert_eq!(derived_dpk.as_string(), "[d1d04177]tpubD6NzVbkrYhZ4WywdEfYbbd62yuvqLjAZuPsNyvzCNV85JekAEMbKHWSHLF9h3j45SxewXDcLv328B1SEZrxg4iwGfmdt1pDFjZiTkGiFqGa/*");
    }

    #[test]
    fn test_derive_descriptors_keys() {
        let master_dsk = get_descriptor_secret_key();
        let derived_dsk: &DescriptorSecretKey = &derive_dsk(&master_dsk, "m/0").unwrap();
        assert_eq!(derived_dsk.as_string(), "[d1d04177/0]tprv8d7Y4JLmD25jkKbyDZXcdoPHu1YtMHuH21qeN7mFpjfumtSU7eZimFYUCSa3MYzkEYfSNRBV34GEr2QXwZCMYRZ7M1g6PUtiLhbJhBZEGYJ/*");

        let master_dpk: &DescriptorPublicKey = &master_dsk.as_public();
        let derived_dpk: &DescriptorPublicKey = &derive_dpk(master_dpk, "m/0").unwrap();
        assert_eq!(derived_dpk.as_string(), "[d1d04177/0]tpubD9oaCiP1MPmQdndm7DCD3D3QU34pWd6BbKSRedoZF1UJcNhEk3PJwkALNYkhxeTKL29oGNR7psqvT1KZydCGqUDEKXN6dVQJY2R8ooLPy8m/*");
    }

    #[test]
    fn test_extend_descriptor_keys() {
        let master_dsk = get_descriptor_secret_key();
        let extended_dsk: &DescriptorSecretKey = &extend_dsk(&master_dsk, "m/0").unwrap();
        assert_eq!(extended_dsk.as_string(), "tprv8ZgxMBicQKsPdWuqM1t1CDRvQtQuBPyfL6GbhQwtxDKgUAVPbxmj71pRA8raTqLrec5LyTs5TqCxdABcZr77bt2KyWA5bizJHnC4g4ysm4h/0/*");

        let master_dpk: &DescriptorPublicKey = &master_dsk.as_public();
        let extended_dpk: &DescriptorPublicKey = &extend_dpk(master_dpk, "m/0").unwrap();
        assert_eq!(extended_dpk.as_string(), "tpubD6NzVbkrYhZ4WywdEfYbbd62yuvqLjAZuPsNyvzCNV85JekAEMbKHWSHLF9h3j45SxewXDcLv328B1SEZrxg4iwGfmdt1pDFjZiTkGiFqGa/0/*");

        let wif = "L2wTu6hQrnDMiFNWA5na6jB12ErGQqtXwqpSL7aWquJaZG8Ai3ch";
        let extended_key = DescriptorSecretKey::from_string(wif.to_string()).unwrap();
        let result = extended_key.derive(Arc::new(DerivationPath::new("m/0".to_string()).unwrap()));
        dbg!(&result);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_descriptor_secret_key() {
        let key1 = "L2wTu6hQrnDMiFNWA5na6jB12ErGQqtXwqpSL7aWquJaZG8Ai3ch";
        let key2 = "tprv8ZgxMBicQKsPcwcD4gSnMti126ZiETsuX7qwrtMypr6FBwAP65puFn4v6c3jrN9VwtMRMph6nyT63NrfUL4C3nBzPcduzVSuHD7zbX2JKVc/1/1/1/*";

        let private_descriptor_key1 = DescriptorSecretKey::from_string(key1.to_string()).unwrap();
        let private_descriptor_key2 = DescriptorSecretKey::from_string(key2.to_string()).unwrap();

        dbg!(private_descriptor_key1);
        dbg!(private_descriptor_key2);

        // Should error out because you can't produce a DescriptorSecretKey from an xpub
        let key0 = "tpubDBrgjcxBxnXyL575sHdkpKohWu5qHKoQ7TJXKNrYznh5fVEGBv89hA8ENW7A8MFVpFUSvgLqc4Nj1WZcpePX6rrxviVtPowvMuGF5rdT2Vi";
        assert!(DescriptorSecretKey::from_string(key0.to_string()).is_err());
    }

    #[test]
    fn test_derive_and_extend_descriptor_secret_key() {
        let master_dsk = get_descriptor_secret_key();
        // derive DescriptorSecretKey with path "m/0" from master
        let derived_dsk: &DescriptorSecretKey = &derive_dsk(&master_dsk, "m/0").unwrap();
        assert_eq!(derived_dsk.as_string(), "[d1d04177/0]tprv8d7Y4JLmD25jkKbyDZXcdoPHu1YtMHuH21qeN7mFpjfumtSU7eZimFYUCSa3MYzkEYfSNRBV34GEr2QXwZCMYRZ7M1g6PUtiLhbJhBZEGYJ/*");

        // extend derived_dsk with path "m/0"
        let extended_dsk: &DescriptorSecretKey = &extend_dsk(derived_dsk, "m/0").unwrap();
        assert_eq!(extended_dsk.as_string(), "[d1d04177/0]tprv8d7Y4JLmD25jkKbyDZXcdoPHu1YtMHuH21qeN7mFpjfumtSU7eZimFYUCSa3MYzkEYfSNRBV34GEr2QXwZCMYRZ7M1g6PUtiLhbJhBZEGYJ/0/*");
    }

    #[test]
    fn test_derive_hardened_path_using_public() {
        let master_dpk = get_descriptor_secret_key().as_public();
        let derived_dpk = &derive_dpk(&master_dpk, "m/84h/1h/0h");
        assert!(derived_dpk.is_err());
    }

    #[test]
    fn test_retrieve_master_secret_key() {
        let master_dpk = get_descriptor_secret_key();
        let master_private_key = master_dpk.secret_bytes().to_hex();
        assert_eq!(
            master_private_key,
            "e93315d6ce401eb4db803a56232f0ed3e69b053774e6047df54f1bd00e5ea936"
        )
    }

    #[test]
    fn test_psbt_fee() {
        let test_wpkh = "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
        let (funded_wallet, _, _) = get_funded_wallet(test_wpkh);
        let test_wallet = Wallet {
            wallet_mutex: Mutex::new(funded_wallet),
        };
        let drain_to_address = "tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt".to_string();
        let drain_to_script = crate::Address::new(drain_to_address)
            .unwrap()
            .script_pubkey();

        let tx_builder = TxBuilder::new()
            .fee_rate(2.0)
            .drain_wallet()
            .drain_to(drain_to_script.clone());
        //dbg!(&tx_builder);
        assert!(tx_builder.drain_wallet);
        assert_eq!(tx_builder.drain_to, Some(drain_to_script.script.clone()));

        let tx_builder_result = tx_builder.finish(&test_wallet).unwrap();

        assert!(tx_builder_result.psbt.fee_rate().is_some());
        assert_eq!(
            tx_builder_result.psbt.fee_rate().unwrap().as_sat_per_vb(),
            2.682927
        );

        assert!(tx_builder_result.psbt.fee_amount().is_some());
        assert_eq!(tx_builder_result.psbt.fee_amount().unwrap(), 220);
    }

    #[test]
    fn test_descriptor_templates() {
        let master: Arc<DescriptorSecretKey> = Arc::new(get_descriptor_secret_key());
        println!("Master: {:?}", master.as_string());
        // tprv8ZgxMBicQKsPdWuqM1t1CDRvQtQuBPyfL6GbhQwtxDKgUAVPbxmj71pRA8raTqLrec5LyTs5TqCxdABcZr77bt2KyWA5bizJHnC4g4ysm4h

        let handmade_public_44 = master
            .derive(Arc::new(
                DerivationPath::new("m/44h/1h/0h".to_string()).unwrap(),
            ))
            .unwrap()
            .as_public();
        println!("Public 44: {}", handmade_public_44.as_string());
        // Public 44: [d1d04177/44'/1'/0']tpubDCoPjomfTqh1e7o1WgGpQtARWtkueXQAepTeNpWiitS3Sdv8RKJ1yvTrGHcwjDXp2SKyMrTEca4LoN7gEUiGCWboyWe2rz99Kf4jK4m2Zmx/*

        let handmade_public_49 = master
            .derive(Arc::new(
                DerivationPath::new("m/49h/1h/0h".to_string()).unwrap(),
            ))
            .unwrap()
            .as_public();
        println!("Public 49: {}", handmade_public_49.as_string());
        // Public 49: [d1d04177/49'/1'/0']tpubDC65ZRvk1NDddHrVAUAZrUPJ772QXzooNYmPywYF9tMyNLYKf5wpKE7ZJvK9kvfG3FV7rCsHBNXy1LVKW95jrmC7c7z4hq7a27aD2sRrAhR/*

        let handmade_public_84 = master
            .derive(Arc::new(
                DerivationPath::new("m/84h/1h/0h".to_string()).unwrap(),
            ))
            .unwrap()
            .as_public();
        println!("Public 84: {}", handmade_public_84.as_string());
        // Public 84: [d1d04177/84'/1'/0']tpubDDNxbq17egjFk2edjv8oLnzxk52zny9aAYNv9CMqTzA4mQDiQq818sEkNe9Gzmd4QU8558zftqbfoVBDQorG3E4Wq26tB2JeE4KUoahLkx6/*

        let template_private_44 =
            Descriptor::new_bip44(master.clone(), KeychainKind::External, Network::Testnet);
        let template_private_49 =
            Descriptor::new_bip49(master.clone(), KeychainKind::External, Network::Testnet);
        let template_private_84 =
            Descriptor::new_bip84(master, KeychainKind::External, Network::Testnet);

        // the extended public keys are the same when creating them manually as they are with the templates
        println!("Template 49: {}", template_private_49.as_string());
        println!("Template 44: {}", template_private_44.as_string());
        println!("Template 84: {}", template_private_84.as_string());

        // for the public versions of the templates these are incorrect, bug report and fix in bitcoindevkit/bdk#817 and bitcoindevkit/bdk#818
        let template_public_44 = Descriptor::new_bip44_public(
            handmade_public_44,
            "d1d04177".to_string(),
            KeychainKind::External,
            Network::Testnet,
        );
        let template_public_49 = Descriptor::new_bip49_public(
            handmade_public_49,
            "d1d04177".to_string(),
            KeychainKind::External,
            Network::Testnet,
        );
        let template_public_84 = Descriptor::new_bip84_public(
            handmade_public_84,
            "d1d04177".to_string(),
            KeychainKind::External,
            Network::Testnet,
        );

        println!("Template public 49: {}", template_public_49.as_string());
        println!("Template public 44: {}", template_public_44.as_string());
        println!("Template public 84: {}", template_public_84.as_string());

        // when using a public key, both as_string and as_string_private return the same string
        assert_eq!(
            template_public_44.as_string_private(),
            template_public_44.as_string()
        );
        assert_eq!(
            template_public_49.as_string_private(),
            template_public_49.as_string()
        );
        assert_eq!(
            template_public_84.as_string_private(),
            template_public_84.as_string()
        );

        // when using as_string on a private key, we get the same result as when using it on a public key
        assert_eq!(
            template_private_44.as_string(),
            template_public_44.as_string()
        );
        assert_eq!(
            template_private_49.as_string(),
            template_public_49.as_string()
        );
        assert_eq!(
            template_private_84.as_string(),
            template_public_84.as_string()
        );
    }

    #[test]
    fn test_descriptor_from_string() {
        let descriptor1 = Descriptor::new("wpkh(tprv8hwWMmPE4BVNxGdVt3HhEERZhondQvodUY7Ajyseyhudr4WabJqWKWLr4Wi2r26CDaNCQhhxEftEaNzz7dPGhWuKFU4VULesmhEfZYyBXdE/0/*)".to_string(), Network::Testnet);
        let descriptor2 = Descriptor::new("wpkh(tprv8hwWMmPE4BVNxGdVt3HhEERZhondQvodUY7Ajyseyhudr4WabJqWKWLr4Wi2r26CDaNCQhhxEftEaNzz7dPGhWuKFU4VULesmhEfZYyBXdE/0/*)".to_string(), Network::Bitcoin);

        // Creating a Descriptor using an extended key that doesn't match the network provided will throw and InvalidNetwork Error
        assert!(descriptor1.is_ok());
        assert_matches!(
            descriptor2.unwrap_err(),
            bdk::Error::Descriptor(Key(InvalidNetwork))
        )
    }

    #[test]
    fn test_wallet_from_descriptor() {
        let descriptor1 = Descriptor::new("wpkh(tprv8hwWMmPE4BVNxGdVt3HhEERZhondQvodUY7Ajyseyhudr4WabJqWKWLr4Wi2r26CDaNCQhhxEftEaNzz7dPGhWuKFU4VULesmhEfZYyBXdE/0/*)".to_string(), Network::Testnet).unwrap();

        let wallet1 = Wallet::new(
            Arc::new(Descriptor::new("wpkh(tprv8hwWMmPE4BVNxGdVt3HhEERZhondQvodUY7Ajyseyhudr4WabJqWKWLr4Wi2r26CDaNCQhhxEftEaNzz7dPGhWuKFU4VULesmhEfZYyBXdE/0/*)".to_string(), Network::Testnet).unwrap()),
            None,
            Network::Testnet,
            DatabaseConfig::Memory
        );

        let wallet2 = Wallet::new(
            Arc::new(descriptor1),
            None,
            Network::Bitcoin,
            DatabaseConfig::Memory,
        );

        // Creating a wallet using a Descriptor with an extended key that doesn't match the network provided in the wallet constructor will throw and InvalidNetwork Error
        assert!(wallet1.is_ok());
        assert_matches!(
            wallet2.unwrap_err(),
            bdk::Error::Descriptor(Key(InvalidNetwork))
        )
    }
}
