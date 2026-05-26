interface HeaderProps {
  version: string;
  gitHash: string;
}

export function Header({ version, gitHash }: HeaderProps) {
  const hasGitHash = gitHash !== "unknown";
  const shortGitHash = hasGitHash ? gitHash.slice(0, 7) : gitHash;

  return (
    <header className="header">
      <div className="header-left">
        <h1 className="header-title">Wakaru Playground</h1>
        <span className="header-subtitle">JavaScript Decompiler</span>
        <span className="header-version">v{version}</span>
        {hasGitHash ? (
          <a
            className="header-commit"
            href={`https://github.com/pionxzh/wakaru/commit/${gitHash}`}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={`View commit ${shortGitHash}`}
          >
            @{shortGitHash}
          </a>
        ) : (
          <span className="header-commit">@unknown</span>
        )}
      </div>
      <div className="header-right">
        <a
          href="https://github.com/pionxzh/wakaru"
          target="_blank"
          rel="noopener noreferrer"
          className="header-link"
        >
          GitHub
        </a>
      </div>
    </header>
  );
}
