use anyhow::Result;
use std::collections::HashMap;
use std::pin::Pin;
use futures::Stream;
use futures::stream::StreamExt;
use tokio::sync::mpsc;
use yellowstone_grpc_proto::prelude::*;

#[derive(Debug, Clone)]
pub struct GeyserTransaction {
    pub signature: String,
}

pub struct GeyserStreamClient {
    pub url: String,
}

impl GeyserStreamClient {
    pub fn new(url: String, _token: Option<String>) -> Self {
        Self { url }
    }

    pub async fn start(&mut self, tx_sender: mpsc::UnboundedSender<GeyserTransaction>) -> Result<()> {
        println!("🔄 Connecting to Geyser at: {}", self.url);
        
        let mut grpc_client = geyser_client::GeyserClient::connect(self.url.clone()).await?;
        
        let request = self.create_request();
        let stream = futures::stream::once(async { request });
        let request: Pin<Box<dyn Stream<Item = SubscribeRequest> + Send + 'static>> = Box::pin(stream);
        
        let response = grpc_client.subscribe(request).await?;
        let mut response_stream = response.into_inner();
        
        println!("✅ Geyser subscribed successfully");
        
        while let Some(update_result) = response_stream.next().await {
            match update_result {
                Ok(update) => {
                    if let Some(update_oneof) = update.update_oneof {
                        if let subscribe_update::UpdateOneof::Transaction(transaction_update) = update_oneof {
                            if let Some(transaction) = transaction_update.transaction {
                                let signature = bs58::encode(&transaction.signature).into_string();
                                let _ = tx_sender.send(GeyserTransaction { signature });
                            }
                        }
                    }
                }
                Err(e) => eprintln!("❌ Geyser stream error: {:?}", e),
            }
        }
        
        eprintln!("🔌 Geyser stream ended");
        Ok(())
    }



    fn create_request(&self) -> SubscribeRequest {
        let mut transactions = HashMap::new();
        transactions.insert(
            "pumpfun".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                signature: None,
                account_include: vec![],
                account_exclude: vec![],
                account_required: vec!["6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string()],
            }
        );

        SubscribeRequest {
            accounts: HashMap::new(),
            slots: HashMap::new(),
            transactions,
            transactions_status: HashMap::new(),
            blocks: HashMap::new(),
            blocks_meta: HashMap::new(),
            entry: HashMap::new(),
            commitment: Some(CommitmentLevel::Processed as i32),
            accounts_data_slice: vec![],
            ping: None,
            from_slot: None,
        }
    }
}