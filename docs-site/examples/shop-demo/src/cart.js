import { fetchPrice } from "./api.js";

export class CartStore {
  items = [];

  async add(sku, quantity) {
    const price = await fetchPrice(sku);
    const existing = this.items.find((item) => item.sku === sku);
    if (existing) {
      existing.quantity += quantity;
    } else {
      this.items.push({ sku, quantity, price });
    }
  }

  total() {
    return this.items.reduce((sum, item) => sum + item.price * item.quantity, 0);
  }

  clear() {
    this.items = [];
  }
}
