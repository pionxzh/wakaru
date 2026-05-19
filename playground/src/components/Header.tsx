export function Header() {
  return (
    <header className="header">
      <div className="header-left">
        <h1 className="header-title">Wakaru Playground</h1>
        <span className="header-subtitle">JavaScript Decompiler</span>
      </div>
      <div className="header-right">
        <a
          href="https://github.com/nicolo-ribaudo/wakaru"
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
