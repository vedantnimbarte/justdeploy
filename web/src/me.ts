import { useQuery } from "@tanstack/react-query";
import { allows, api, type Person, type Role } from "./api";

/** The signed-in person, and whether their role allows something. */
export function useMe() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => api.get<Person>("/auth/me"), staleTime: 60_000 });
  return { me: me.data, can: (needed: Role) => allows(me.data?.role, needed) };
}
