export async function fetchPrice(sku) {
  const response = await fetch(`/api/price/${sku}`);
  if (!response.ok) {
    throw new Error(`Price lookup failed: ${response.status}`);
  }
  const { price } = await response.json();
  return price ?? 0;
}
