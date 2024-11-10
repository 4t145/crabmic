use std::borrow::Cow;
pub type Result<T> = std::result::Result<T, ClientError>;
#[derive(Debug)]
pub struct ClientError {
    context: Cow<'static, str>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.context)?;
        if let Some(source) = &self.source {
            write!(f, ": {}", source)?;
        }
        Ok(())
    }
}
impl std::error::Error for ClientError {
    
}
impl ClientError {
    pub fn local(context: impl Into<Cow<'static, str>>) -> Self {
        Self {
            context: context.into(),
            source: None,
        }
    }
    pub fn contextual<E>(context: impl Into<Cow<'static, str>>) -> impl FnOnce(E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        |source| Self {
            context: context.into(),
            source: Some(Box::new(source)),
        }
    }
}
