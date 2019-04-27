use bytes::{Buf, Bytes, BytesMut};

use super::frame::{HttpFrame, SettingsFrame};
use super::{ErrorCode, StreamType};
use crate::qpack::{self, DynamicTable};

#[derive(Debug, PartialEq)]
pub enum ConnectionError {
    SettingsFrameUnexpected,
    SettingsFrameMissing,
    ControlStreamAlreadyOpen,
    EncoderStreamAlreadyOpen,
    DecoderStreamAlreadyOpen,
    UnimplementedStream(StreamType),
    DecoderStreamError { reason: qpack::EncoderError },
    EncoderStreamError { reason: qpack::DecoderError },
}

pub enum State {
    Open,
    Closing(ErrorCode),
    Closed(),
}

pub struct Connection {
    state: State,
    remote_control_stream: bool,
    encoder_stream: bool,
    decoder_stream: bool,
    local_settings: SettingsFrame,
    remote_settings: Option<SettingsFrame>,
    decoder_table: DynamicTable,
    encoder_table: DynamicTable,
    pending_control: BytesMut,
    pending_decoder: BytesMut,
    pending_encoder: BytesMut,
}

impl Connection {
    pub fn new() -> Self {
        let local_settings = SettingsFrame::default();
        let mut pending_control = BytesMut::new();

        local_settings.encode(&mut pending_control);

        Self {
            state: State::Open,
            remote_control_stream: false,
            encoder_stream: false,
            decoder_stream: false,
            local_settings,
            remote_settings: None,
            decoder_table: DynamicTable::new(),
            encoder_table: DynamicTable::new(),
            pending_control,
            pending_decoder: BytesMut::new(),
            pending_encoder: BytesMut::new(),
        }
    }

    pub fn on_recv_control(&mut self, frame: &HttpFrame) {
        match (&self.remote_settings, frame) {
            (None, HttpFrame::Settings(s)) => {
                println!("recieved settings : {:?}", s);
                self.remote_settings = Some(s.to_owned()); // TODO check validity?
            }
            (None, _) => self.state = State::Closing(ErrorCode::MissingSettings),
            (Some(_), HttpFrame::Settings(_)) => {
                self.state = State::Closing(ErrorCode::UnexpectedFrame)
            }
            (Some(_), f) => match f {
                HttpFrame::Priority(_)
                | HttpFrame::CancelPush(_)
                | HttpFrame::Goaway(_)
                | HttpFrame::MaxPushId(_) => {
                    unimplemented!("TODO: unimplemented frame on control stream: {:?}", f)
                }
                _ => self.state = State::Closing(ErrorCode::UnexpectedFrame),
            },
        }
    }

    pub fn on_recv_decoder<T: Buf>(&mut self, buf: &mut T) -> Result<(), ConnectionError> {
        match qpack::on_decoder_recv(&mut self.encoder_table, buf) {
            Err(err) => {
                self.state = State::Closing(ErrorCode::QpackDecoderStreamError);
                Err(ConnectionError::DecoderStreamError { reason: err })
            }
            Ok(_) => Ok(()),
        }
    }

    pub fn on_recv_encoder<R: Buf>(&mut self, encoder: &mut R) -> Result<(), ConnectionError> {
        let ret = qpack::on_encoder_recv(
            &mut self.decoder_table.inserter(),
            encoder,
            &mut self.pending_decoder,
        );
        match ret {
            Err(err) => {
                self.state = State::Closing(ErrorCode::QpackEncoderStreamError);
                Err(ConnectionError::EncoderStreamError { reason: err })
            }
            Ok(_) => Ok(()),
        }
    }

    pub fn on_recv_stream(&mut self, ty: StreamType) -> Result<(), ConnectionError> {
        match ty {
            StreamType::CONTROL => {
                if self.remote_control_stream {
                    self.state = State::Closing(ErrorCode::WrongStreamCount);
                    Err(ConnectionError::ControlStreamAlreadyOpen)
                } else {
                    self.remote_control_stream = true;
                    Ok(())
                }
            }
            StreamType::ENCODER => {
                if self.encoder_stream {
                    self.state = State::Closing(ErrorCode::WrongStreamCount);
                    Err(ConnectionError::EncoderStreamAlreadyOpen)
                } else {
                    self.encoder_stream = true;
                    Ok(())
                }
            }
            StreamType::DECODER => {
                if self.decoder_stream {
                    self.state = State::Closing(ErrorCode::WrongStreamCount);
                    Err(ConnectionError::DecoderStreamAlreadyOpen)
                } else {
                    self.decoder_stream = true;
                    Ok(())
                }
            }
            _ => {
                self.state = State::Closing(ErrorCode::UnknownStreamType);
                Err(ConnectionError::UnimplementedStream(ty))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_recv_stream_unique(ty: StreamType, err: ConnectionError) {
        let mut conn = Connection::new();
        assert_eq!(conn.on_recv_stream(ty), Ok(()));
        assert_eq!(conn.on_recv_stream(ty), Err(err));
    }

    #[test]
    fn recv_stream() {
        check_recv_stream_unique(
            StreamType::CONTROL,
            ConnectionError::ControlStreamAlreadyOpen,
        );
        check_recv_stream_unique(
            StreamType::ENCODER,
            ConnectionError::EncoderStreamAlreadyOpen,
        );
        check_recv_stream_unique(
            StreamType::DECODER,
            ConnectionError::DecoderStreamAlreadyOpen,
        );
    }

    #[test]
    fn handle_settings_frame() {
        let mut conn = Connection::new();

        let settings = SettingsFrame::default();
        conn.on_recv_control(&HttpFrame::Settings(settings.clone()));
        assert_eq!(Some(settings), conn.remote_settings);
    }
}