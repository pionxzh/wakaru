export async function checkout(cart) {
  const response = await fetch("/api/checkout", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ items: cart.items }),
  });
  window.location.href = response.ok ? "/thanks" : "/cart?error=checkout";
}
