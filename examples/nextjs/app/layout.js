export const metadata = { title: "Next.js on justdeploy" };

export default function RootLayout({ children }) {
  return (
    <html lang="en">
      <body style={{ margin: 0, background: "#f1efed", color: "#161513", fontFamily: "system-ui, sans-serif" }}>
        {children}
      </body>
    </html>
  );
}
