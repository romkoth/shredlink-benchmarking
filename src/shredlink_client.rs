use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use shredlink_proto::shredlink::{SubscribeTransactionsRequest, SubscribeRequestFilterTransactions};
use shredlink_proto::shredlink::shredlink_service_client::ShredlinkServiceClient;

// These should be generated from your actual Shredlink protobuf files
// Based on your TypeScript usage

#[derive(Debug, Clone)]
pub struct ShredlinkTransaction {
    pub signatures: Vec<Vec<u8>>,
}

pub struct ShredlinkClient {
    pub url: String,
}

impl ShredlinkClient {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub async fn start(&mut self, tx_sender: mpsc::UnboundedSender<ShredlinkTransaction>) -> Result<()> {
        println!("🔄 Connecting to Shredlink at: {}", self.url);
        
        let channel = tonic::transport::Endpoint::from_shared(self.url.clone())?
            .connect().await?;
        let mut client = ShredlinkServiceClient::new(channel);
        
        // Create request/response channels for streaming
        let (subscribe_tx, subscribe_rx) = tokio::sync::mpsc::unbounded_channel();
        
        // Start subscription
        let response = client.subscribe_transactions(UnboundedReceiverStream::new(subscribe_rx)).await?;
        let mut stream = response.into_inner();
        
        // Send the subscribe request
        let request = self.create_request();
        let _ = subscribe_tx.send(request);
        
        println!("✅ Shredlink subscribed successfully");
        
        // Handle incoming transaction stream
        while let Some(message) = stream.message().await? {
            if let Some(transaction_update) = message.transaction {
                if let Some(transaction) = transaction_update.transaction {
                    let shredlink_tx = ShredlinkTransaction {
                        signatures: transaction.signatures,
                    };
                    
                    let _ = tx_sender.send(shredlink_tx);
                }
            }
        }
        
        eprintln!("🔌 Shredlink stream ended");
        Ok(())
    }

    fn create_request(&self) -> SubscribeTransactionsRequest {
        let mut transactions = HashMap::new();
        transactions.insert(
            "pumpfun".to_string(),
            SubscribeRequestFilterTransactions {
                account_include: vec![],
                account_exclude: vec![],
                account_required: vec!["6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string()],
            }
        );

        SubscribeTransactionsRequest { 
            transactions,
            request_type: None,
        }
    }
}