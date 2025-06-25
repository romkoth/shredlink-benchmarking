use anyhow::Result;
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;
use yellowstone_grpc_client::{GeyserGrpcClient, Interceptor, ClientTlsConfig};
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};

#[derive(Debug, Clone)]
pub struct GeyserTransaction {
    pub signature: String,
    pub slot: u64,
}

pub struct GeyserStreamClient {
    endpoint: String,
    token: Option<String>,
}

impl GeyserStreamClient {
    pub fn new(endpoint: String, token: Option<String>) -> Self {
        Self { endpoint, token }
    }

    async fn create_client(&self) -> Result<GeyserGrpcClient<impl Interceptor>> {
        let mut builder = GeyserGrpcClient::build_from_shared(self.endpoint.clone())?;
        
        // Configure TLS for HTTPS endpoints
        if self.endpoint.starts_with("https://") {
            builder = builder.tls_config(ClientTlsConfig::new().with_native_roots())?;
        }
        
        // Configure message size limit
        builder = builder.max_decoding_message_size(1024 * 1024 * 1024);
        
        // Add token authentication if provided
        if let Some(token) = &self.token {
            builder = builder.x_token(Some(token.clone()))?;
        }

        let client = builder.connect().await
            .map_err(|e| anyhow::anyhow!("gRPC connection failed: {}", e))?;
        
        Ok(client)
    }

    pub async fn start(&mut self, tx: UnboundedSender<GeyserTransaction>) -> Result<()> {
        let mut client = self.create_client().await?;
        
        // Create subscription request for all transactions
        let mut transactions = HashMap::new();
        transactions.insert(
            "transactions".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                signature: None,
                account_include: vec![],
                account_exclude: vec![],
                account_required: vec!["6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string()],
            }
        );

        let request = SubscribeRequest {
            slots: HashMap::new(),
            accounts: HashMap::new(),
            transactions,
            transactions_status: HashMap::new(),
            entry: HashMap::new(),
            blocks: HashMap::new(),
            blocks_meta: HashMap::new(),
            commitment: Some(CommitmentLevel::Processed as i32),
            accounts_data_slice: vec![],
            ping: None,
            from_slot: None,
        };

        let (mut _subscribe_tx, mut stream) = client.subscribe_with_request(Some(request)).await
            .map_err(|e| anyhow::anyhow!("Failed to start subscription: {}", e))?;

        // Process the stream
        while let Some(message) = stream.next().await {
            match message {
                Ok(msg) => {
                    if let Some(UpdateOneof::Transaction(transaction_update)) = msg.update_oneof {
                        if let Some(transaction) = transaction_update.transaction {
                            let signature = bs58::encode(&transaction.signature).into_string();
                            
                            let geyser_transaction = GeyserTransaction {
                                signature,
                                slot: transaction_update.slot,
                            };

                            if let Err(e) = tx.send(geyser_transaction) {
                                eprintln!("❌ Failed to send Geyser transaction: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("❌ Geyser stream error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}