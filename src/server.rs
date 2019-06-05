use std::io::prelude::*;

pub struct Server {
    inner: std::sync::Arc<ServerInner>,
}

struct ServerInner {
    primary_endpoint: Endpoint,
    secondary_endpoint: Endpoint,
    resolver: trust_dns_resolver::Resolver,
}

struct Endpoint {
    host: String,
    port: u16,
}

impl Server {
    pub fn new(
        primary_endpoint_str: &str,
        secondary_endpoint_str: &str,
    ) -> Result<Self, failure::Error> {
        let resolver = trust_dns_resolver::Resolver::new(
            trust_dns_resolver::config::ResolverConfig::default(),
            trust_dns_resolver::config::ResolverOpts::default(),
        )?;
        let primary_endpoint = decompose_endpoint_str(primary_endpoint_str)?;
        let secondary_endpoint = decompose_endpoint_str(secondary_endpoint_str)?;

        Ok(Self {
            inner: std::sync::Arc::new(ServerInner {
                primary_endpoint,
                secondary_endpoint,
                resolver,
            }),
        })
    }

    pub fn listen(&self) -> Result<(), failure::Error> {
        let listener = std::net::TcpListener::bind("0.0.0.0:11211")?; // TODO: Should we make this configurable?

        // TODO: It may cause unlimited spawing threads. It is better to re-implement this with thread pool.
        for stream in listener.incoming() {
            let local_self = self.inner.clone();
            std::thread::spawn(move || {
                match local_self.handle_incomming_connection(stream) {
                    Ok(_) => { /* do nothing */ }
                    Err(e) => {
                        log::error!("{:?}", e);
                    }
                }
            });
        }

        Ok(())
    }

}

impl ServerInner {
    fn handle_incomming_connection(
        &self,
        stream: Result<std::net::TcpStream, std::io::Error>,
    ) -> Result<(), failure::Error> {
        let client_stream = stream?;
        let (primary_ip_addr_opt, secondary_ip_addr_opt) =
            self.resolve_configuration_endpoints()?;
        let primary_upstream = match primary_ip_addr_opt {
            Some(primary_ip_addr) => std::net::TcpStream::connect(format!(
                "{}:{}",
                primary_ip_addr, self.primary_endpoint.port,
            ))
            .ok(),
            None => {
                log::warn!("It seems no primary cluster endpoint");
                None
            }
        };
        let secondary_upstream = match secondary_ip_addr_opt {
            Some(secondary_ip_addr) => std::net::TcpStream::connect(format!(
                "{}:{}",
                secondary_ip_addr, self.secondary_endpoint.port,
            ))
            .ok(),
            None => {
                log::warn!("It seems no secondary cluster endpoint");
                None
            }
        };

        let mut client_stream_reader = std::io::BufReader::new(&client_stream);
        let mut client_stream_writer = std::io::BufWriter::new(&client_stream);
        let mut client_stream_read_buffer = String::with_capacity(1 * 1024 * 1024);

        if primary_upstream.is_none() && secondary_upstream.is_none() {
            client_stream_writer
                .write_all(b"SERVER_ERROR There is no available configuration endpoint.\r\n")?;
            client_stream_writer.flush()?;

            return Err(failure::format_err!(
                "There is no available configuration endpoint."
            ));
        }

        loop {
            let client_request_len =
                client_stream_reader.read_line(&mut client_stream_read_buffer)?;
            if client_request_len == 0 {
                // Reached to EOF. It means that peer's connection is closed.
                break;
            }
            log::debug!("{}", client_stream_read_buffer);
            if client_stream_read_buffer.starts_with("stats") {
                log::debug!("Proxy stats command");
                self.handle_stats_command(
                    &primary_upstream,
                    &secondary_upstream,
                    &client_stream_read_buffer,
                    &mut client_stream_writer,
                )?;
            } else if client_stream_read_buffer.starts_with("config get cluster") {
                log::debug!("Proxy config get cluster command");
                self.handle_config_get_cluster_command(
                    &primary_upstream,
                    &secondary_upstream,
                    &client_stream_read_buffer,
                    &mut client_stream_writer,
                )?;
            } else {
                log::debug!("Unknown command.");
                client_stream_writer.write_all(b"SERVER_ERROR mimikyu proxy supports only `config get cluser` and `stats` commands\r\n")?;
                client_stream_writer.flush()?;

                return Err(failure::format_err!(
                    "A client sent a non-supported command: {}",
                    client_stream_read_buffer
                ));
            }

            // Ensure clear all buffer strings for next client interuction.
            client_stream_read_buffer.clear();
        }

        Ok(())
    }

    fn resolve_configuration_endpoints(
        &self,
    ) -> Result<(Option<std::net::IpAddr>, Option<std::net::IpAddr>), failure::Error> {
        let primary_response = match self.resolver.lookup_ip(&self.primary_endpoint.host) {
            Ok(r) => r,
            Err(e) => return Err(failure::format_err!("{:?}", e)),
        };
        let secondary_response = match self.resolver.lookup_ip(&self.secondary_endpoint.host) {
            Ok(r) => r,
            Err(e) => return Err(failure::format_err!("{:?}", e)),
        };

        /* Return first A records */
        Ok((
            primary_response.iter().next(),
            secondary_response.iter().next(),
        ))
    }

    fn handle_stats_command(
        &self,
        primary_upstream: &Option<std::net::TcpStream>,
        secondary_upstream: &Option<std::net::TcpStream>,
        client_stream_read_buffer: &str,
        client_stream_writer: &mut std::io::BufWriter<&std::net::TcpStream>,
    ) -> Result<(), failure::Error> {
        if primary_upstream.is_some() {
            let primary_upstream_inner = primary_upstream.as_ref().unwrap();
            let mut primary_upstream_writer = std::io::BufWriter::new(primary_upstream_inner);
            let mut primary_upstream_reader = std::io::BufReader::new(primary_upstream_inner);
            let mut primary_upstream_read_buffer = String::with_capacity(1 * 1024 * 1024);

            primary_upstream_writer.write_all(&client_stream_read_buffer.as_bytes())?;
            primary_upstream_writer.flush()?;

            loop {
                primary_upstream_reader.read_line(&mut primary_upstream_read_buffer)?;
                log::debug!("{}", primary_upstream_read_buffer);
                if primary_upstream_read_buffer.ends_with("END\r\n") {
                    break;
                }
            }

            client_stream_writer.write_all(&primary_upstream_read_buffer.as_bytes())?;
            client_stream_writer.flush()?;
        } else if secondary_upstream.is_some() {
            let secondary_upstream_inner = secondary_upstream.as_ref().unwrap();
            let mut secondary_upstream_writer = std::io::BufWriter::new(secondary_upstream_inner);
            let mut secondary_upstream_reader = std::io::BufReader::new(secondary_upstream_inner);
            let mut secondary_upstream_read_buffer = String::with_capacity(1 * 1024 * 1024);

            secondary_upstream_writer.write_all(&client_stream_read_buffer.as_bytes())?;
            secondary_upstream_writer.flush()?;

            loop {
                secondary_upstream_reader.read_line(&mut secondary_upstream_read_buffer)?;
                log::debug!("{}", secondary_upstream_read_buffer);
                if secondary_upstream_read_buffer.ends_with("END\r\n") {
                    break;
                }
            }

            client_stream_writer.write_all(&secondary_upstream_read_buffer.as_bytes())?;
            client_stream_writer.flush()?;
        } else {
            unreachable!();
        }

        Ok(())
    }

    // Merge responses from upstreams like following example.secondary_upstream_read_buffer
    //
    // ```
    // CONFIG cluster 0 156
    // 1
    // mozamimy-cluster-001.qenso7.0001.apne1.cache.amazonaws.com|10.18.5.229|11211 mozamimy-cluster-001.qenso7.0002.apne1.cache.amazonaws.com|10.18.27.74|11211
    //
    // END
    // ```
    fn handle_config_get_cluster_command(
        &self,
        primary_upstream: &Option<std::net::TcpStream>,
        secondary_upstream: &Option<std::net::TcpStream>,
        client_stream_read_buffer: &str,
        client_stream_writer: &mut std::io::BufWriter<&std::net::TcpStream>,
    ) -> Result<(), failure::Error> {
        let mut modification_count = 0;
        let mut clusters_line = String::with_capacity(512);
        let mut current_line: u8 = 0;

        if primary_upstream.is_some() {
            let primary_upstream_innner = primary_upstream.as_ref().unwrap();
            let mut primary_upstream_writer = std::io::BufWriter::new(primary_upstream_innner);
            let mut primary_upstream_reader = std::io::BufReader::new(primary_upstream_innner);
            let mut primary_upstream_read_buffer = String::with_capacity(1 * 1024 * 1024);

            primary_upstream_writer.write_all(&client_stream_read_buffer.as_bytes())?;
            primary_upstream_writer.flush()?;

            loop {
                primary_upstream_reader.read_line(&mut primary_upstream_read_buffer)?;
                log::debug!("{}", primary_upstream_read_buffer);
                if primary_upstream_read_buffer.starts_with("END") {
                    break;
                }
                match current_line {
                    1 => {
                        let m: u8 = primary_upstream_read_buffer.trim().parse()?;
                        modification_count += m;
                    }
                    2 => {
                        clusters_line.push_str(primary_upstream_read_buffer.trim());
                    }
                    _ => { /* do nothings */ }
                }
                current_line += 1;
                primary_upstream_read_buffer.clear();
            }

            clusters_line.push_str(" ");
        }

        if secondary_upstream.is_some() {
            let secondary_upstream_inner = secondary_upstream.as_ref().unwrap();
            let mut secondary_upstream_writer = std::io::BufWriter::new(secondary_upstream_inner);
            let mut secondary_upstream_reader = std::io::BufReader::new(secondary_upstream_inner);
            let mut secondary_upstream_read_buffer = String::with_capacity(1 * 1024 * 1024);

            secondary_upstream_writer.write_all(&client_stream_read_buffer.as_bytes())?;
            secondary_upstream_writer.flush()?;
            current_line = 0;

            loop {
                secondary_upstream_reader.read_line(&mut secondary_upstream_read_buffer)?;
                log::debug!("{}", secondary_upstream_read_buffer);
                if secondary_upstream_read_buffer.starts_with("END") {
                    break;
                }
                match current_line {
                    2 => {
                        clusters_line.push_str(secondary_upstream_read_buffer.trim());
                    }
                    _ => { /* do nothings */ }
                }
                current_line += 1;
                secondary_upstream_read_buffer.clear();
            }
        }


        let mut client_response_body = String::with_capacity(1024);
        client_response_body.push_str(&modification_count.to_string());
        client_response_body.push_str("\r\n");
        client_response_body.push_str(&clusters_line);
        client_response_body.push_str("\r\n\r\n");
        client_response_body.push_str("END\r\n");
        let client_response_body_size = client_response_body.as_bytes().len();
        let client_response = format!(
            "CONFIG cluster 0 {}\r\n{}",
            client_response_body_size, client_response_body
        );
        log::debug!("{}", client_response);

        client_stream_writer.write_all(client_response.as_bytes())?;
        client_stream_writer.flush()?;
        Ok(())
    }
}

fn decompose_endpoint_str(endpoint_str: &str) -> Result<Endpoint, failure::Error> {
    let mut iter = endpoint_str.split(':');
    let host = match iter.next() {
        Some(h) => h,
        None => {
            return Err(failure::format_err!(
                "Invalid endpoint format: {}",
                endpoint_str
            ))
        }
    };
    let port = match iter.next() {
        Some(p) => p,
        None => {
            return Err(failure::format_err!(
                "Invalid endpoint format: {}",
                endpoint_str
            ))
        }
    };

    Ok(Endpoint {
        host: host.to_string(),
        port: port.parse()?,
    })
}
