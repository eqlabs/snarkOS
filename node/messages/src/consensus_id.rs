// Copyright (C) 2019-2023 Aleo Systems Inc.
// This file is part of the snarkOS library.

// The snarkOS library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The snarkOS library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the snarkOS library. If not, see <https://www.gnu.org/licenses/>.

use narwhal_crypto::{PublicKey, Signature};

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsensusId {
    pub public_key: PublicKey,
    pub signature: Signature,
}

impl MessageTrait for Box<ConsensusId> {
    fn name(&self) -> String {
        "ConsensusId".to_string()
    }

    fn serialize<W: Write>(&self, writer: &mut W) -> Result<()> {
        bincode::serialize_into(writer, &(&self.public_key, &self.signature))?;
        // serde_json::to_writer(writer.by_ref(), &(&self.public_key, &self.signature))?;

        Ok(())
    }

    fn deserialize(bytes: BytesMut) -> Result<Self> {
        let mut reader = bytes.reader();
        // let (public_key, signature) = bincode::deserialize_from(&mut reader.by_ref())?;
        let mut dst = [0; 1024];
        let num = reader.read(&mut dst).unwrap();
        let (public_key, signature) = bincode::deserialize(&dst[..num])?;

        Ok(Box::new(ConsensusId { public_key, signature }))
    }
}

#[cfg(test)]
mod test {
    use bytes::BufMut;
    use narwhal_crypto::KeyPair as NarwhalKeyPair;

    use super::*;

    #[test]
    fn consensus_id_serialization() {
        let mut rng = rand::thread_rng();
        let keypair = NarwhalKeyPair::new(&mut rng).unwrap();
        let public = keypair.public();
        println!("public: {:?}", public.to_bytes_le().unwrap().len());
        let private = keypair.private();

        let message = &[0u8; 32];
        let signature = private.sign_bytes(message, &mut rng).unwrap();
        println!("signature: {:?}", signature.to_bytes_le().unwrap().len());

        let id = Box::new(ConsensusId { public_key: public.clone(), signature });
        let mut buf = BytesMut::with_capacity(128).writer();
        id.serialize(&mut buf).unwrap();
        let bytes = buf.into_inner();
        let deserialized = MessageTrait::deserialize(bytes).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn signature_serialization() {
        let mut rng = rand::thread_rng();
        let keypair = NarwhalKeyPair::new(&mut rng).unwrap();
        let private = keypair.private();

        let message = &[0u8; 32];
        let signature = private.sign_bytes(message, &mut rng).unwrap();
        let json = serde_json::to_string(&signature).unwrap();
        let deserialized: Signature = serde_json::from_str(&json).unwrap();
        assert_eq!(signature, deserialized);

        // TODO: why does the below fail?
        // let mut buf = BytesMut::with_capacity(256).writer();
        // bincode::serialize_into(&mut buf.by_ref(), &signature).unwrap();
        // let bytes = buf.into_inner();
        // let deserialized: Signature = bincode::deserialize_from(&mut bytes.reader()).unwrap();
        // assert_eq!(signature, deserialized);
    }
}
