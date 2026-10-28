use std::error::Error;

/// `error`'s message followed by those of its sources, as `error: source: …`.
/// reqwest's own message stops at "error sending request for url (…)"; what
/// went wrong (DNS, refused connection, rejected certificate…) is in its
/// sources. A source whose message is already shown is skipped, since some
/// wrappers repeat the message of the error they wrap.
pub(crate) fn with_sources(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        let message = error.to_string();
        if !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        source = error.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Layer {
        message: &'static str,
        source: Option<Box<Layer>>,
    }

    impl Layer {
        fn new(message: &'static str, source: Option<Layer>) -> Self {
            Self {
                message,
                source: source.map(Box::new),
            }
        }
    }

    impl fmt::Display for Layer {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.message)
        }
    }

    impl Error for Layer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source.as_deref().map(|source| source as _)
        }
    }

    #[test]
    fn appends_every_source() {
        let error = Layer::new(
            "error sending request",
            Some(Layer::new(
                "client error (Connect)",
                Some(Layer::new("invalid peer certificate: UnknownIssuer", None)),
            )),
        );
        assert_eq!(
            with_sources(&error),
            "error sending request: client error (Connect): invalid peer certificate: UnknownIssuer"
        );
    }

    #[test]
    fn skips_sources_already_shown() {
        let error = Layer::new(
            "error sending request",
            Some(Layer::new(
                "error sending request",
                Some(Layer::new("timed out", None)),
            )),
        );
        assert_eq!(with_sources(&error), "error sending request: timed out");
    }
}
