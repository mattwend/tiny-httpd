pub(super) async fn shutdown_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = sigterm.recv() => Ok(()),
            result = tokio::signal::ctrl_c() => result,
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
