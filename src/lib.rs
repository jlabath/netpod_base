use bendy::decoding::FromBencode;
use bendy::encoding::ToBencode;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::result::Result;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
pub mod error;
mod impls;
use error::NetpodError;

#[derive(Debug, PartialEq)]
enum Op {
    Describe,
    Invoke,
}

impl Op {
    fn from_str(s: &str) -> Result<Op, String> {
        match s {
            "describe" => Ok(Op::Describe),
            "invoke" => Ok(Op::Invoke),
            _ => Err(format!("Invalid operation: {}", s)),
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct Request {
    op: Op,
    pub id: Option<String>,
    var: Option<String>,
    pub args: Option<String>,
}

#[derive(PartialEq, Debug)]
struct Var {
    name: String,
}

#[derive(PartialEq, Debug)]
struct Namespace {
    name: String,
    vars: Vec<Var>,
}

#[derive(PartialEq, Debug)]
pub struct DescribeResponse {
    format: String,
    namespaces: Vec<Namespace>,
}

#[derive(Debug, PartialEq)]
pub enum Status {
    Done,
    Error,
}

impl Status {
    fn as_str(&self) -> &str {
        match self {
            Self::Done => "done",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ErrorResponse {
    id: Option<String>,
    status: Status,
    ex_message: String,
    //ex_data: Option<String>,
}

pub fn err_response(id: Option<String>, err: NetpodError) -> Response {
    Response::Error(ErrorResponse {
        id,
        status: Status::Error,
        ex_message: err.to_string(),
    })
}

pub fn invoke_response(id: String, value: Vec<u8>) -> Response {
    let r = InvokeResponse {
        id,
        status: Status::Done,
        value,
    };
    Response::Invoke(r)
}

#[derive(PartialEq, Debug)]
pub struct InvokeResponse {
    id: String,
    status: Status,
    value: Vec<u8>,
}

#[derive(Debug)]
pub enum Response {
    Describe(DescribeResponse),
    Invoke(InvokeResponse),
    Error(ErrorResponse),
}

async fn read_request(stream: &mut UnixStream) -> Result<Request, NetpodError> {
    let mut buffer = [0; 1024 * 2];
    let mut data = Vec::new();
    let req: Option<Request>;

    loop {
        let bytes_read = stream.read(&mut buffer).await?;

        if bytes_read == 0 {
            req = Some(decode_request(&data)?);
            break; // End of stream reached
        }

        // Append the read data
        data.extend_from_slice(&buffer[..bytes_read]);

        match decode_request(&data) {
            Ok(r) => {
                req = Some(r);
                break;
            }
            Err(_e) => continue,
        }
    }

    req.ok_or("request is None".into())
}

fn decode_request(buffer: &[u8]) -> Result<Request, NetpodError> {
    // Check if the last byte is `e` (ASCII value for 'e') which marks dictionary termination
    if buffer[buffer.len() - 1] == b'e' {
        Request::from_bencode(buffer).map_err(NetpodError::from)
    } else {
        Err("keep reading".into())
    }
}

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<Response, NetpodError>> + Send>>;

pub type HandlerFn = Box<dyn Fn(Request) -> HandlerFuture + Send + Sync>;

pub async fn run_server(
    socket_path: &str,
    handler_map: HashMap<String, HandlerFn>,
) -> Result<(), NetpodError> {
    // Create the Unix listener
    let listener = UnixListener::bind(socket_path)?;
    let handlers = Arc::new(handler_map);

    // Accept incoming connections
    loop {
        let (stream, _addr) = listener.accept().await?;

        let hm = handlers.clone();
        // Spawn a task to handle the connection
        tokio::spawn(async move { handle_client(stream, hm).await });
    }
}

async fn handle_client(mut stream: UnixStream, handler_map: Arc<HashMap<String, HandlerFn>>) {
    let request = read_request(&mut stream).await;

    match request {
        Ok(req) => {
            let response = handle_request(handler_map, req).await;
            match response {
                Ok(response) => match response.to_bencode() {
                    Ok(buf) => {
                        if let Err(err) = stream.write_all(&buf).await {
                            eprintln!("writing out stream failed {}", err);
                        }
                    }
                    Err(err) => {
                        let er = err_response(None, err.into());
                        if let Ok(e_buf) = er.to_bencode() {
                            if let Err(err) = stream.write_all(&e_buf).await {
                                eprintln!("failed writing out err stream {}", err);
                            }
                        }
                    }
                },
                Err(e) => {
                    eprintln!("handle_request failed with `{}`", e);
                    let er = err_response(None, e);
                    match er.to_bencode() {
                        Ok(e_buf) => {
                            if let Err(err) = stream.write_all(&e_buf).await {
                                eprintln!("failed writing out stream {}", err);
                            }
                        }
                        Err(err) => {
                            eprintln!("trouble encoding error response {}", err);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("trouble reading request from the stream {}", e);
        }
    }
}

async fn handle_request(
    handler_map: Arc<HashMap<String, HandlerFn>>,
    req: Request,
) -> Result<Response, NetpodError> {
    match req.op {
        Op::Describe => handle_describe(handler_map),
        Op::Invoke => handle_invoke(handler_map, req).await,
    }
}

fn handle_describe(handler_map: Arc<HashMap<String, HandlerFn>>) -> Result<Response, NetpodError> {
    /* the describe response look like this
    DescribeResponse{format: "json", namespaces: Vec[Namespace]}
    where Namespace {name: String, vars: Vec[Var]}
    */
    let mut name_map: HashMap<&str, Namespace> = HashMap::new();
    for full_name in handler_map.keys() {
        if let Some((namespace_name, var_name)) = full_name.split_once("/") {
            name_map
                .entry(namespace_name)
                .and_modify(|ns| {
                    let var = Var {
                        name: var_name.to_string(),
                    };
                    ns.vars.push(var)
                })
                .or_insert_with(|| {
                    let var = Var {
                        name: var_name.to_string(),
                    };
                    Namespace {
                        name: namespace_name.to_string(),
                        vars: vec![var],
                    }
                });
        } else {
            eprintln!("invalid name in handler_map {}", full_name);
        }
    }
    let r = DescribeResponse {
        format: "json".to_string(),
        namespaces: name_map.into_values().collect(),
    };
    Ok(Response::Describe(r))
}

async fn handle_invoke(
    handler_map: Arc<HashMap<String, HandlerFn>>,
    req: Request,
) -> Result<Response, NetpodError> {
    if let Some(var_name) = &req.var {
        if let Some(func) = handler_map.get(var_name) {
            func(req).await
        } else {
            eprintln!("handler for {} not found", var_name);
            Ok(err_response(
                req.id,
                NetpodError::Message(format!("error no handler for {}", var_name)),
            ))
        }
    } else {
        eprintln!("request lacks var {:?}", &req);
        Ok(err_response(
            req.id,
            NetpodError::Message("request lacks var name".into()),
        ))
    }
}
