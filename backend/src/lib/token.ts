import crypto from "crypto";

/**
 * Generate a cryptographically secure, URL-safe token.
 * 32 random bytes → 43-character base64url string.
 */
export function generateToken(): string {
  return crypto.randomBytes(32).toString("base64url");
}

/**
 * Return a Date that is `minutes` from now.
 */
export function expiresInMinutes(minutes: number): Date {
  return new Date(Date.now() + minutes * 60 * 1000);
}
