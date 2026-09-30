// Русские склонения: 1 запись / 2 записи / 5 записей. Нужны в нескольких
// страницах для подписей в шапке, поэтому вынесены из `ResultsPage`.

export function pluralish(n: number, one: string, few: string, many: string): string {
  const a = Math.abs(Math.trunc(n)) % 100;
  const b = a % 10;
  if (a > 10 && a < 20) return many;
  if (b > 1 && b < 5) return few;
  if (b === 1) return one;
  return many;
}
