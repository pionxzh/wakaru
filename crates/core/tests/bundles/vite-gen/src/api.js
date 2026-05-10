const BASE_URL = 'https://api.example.com';

function request(path) {
  return fetch(`${BASE_URL}${path}`).then((res) => {
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json();
  });
}

export function getUser(id) {
  return request(`/users/${id}`);
}

export function getPosts(userId) {
  return request(`/users/${userId}/posts`);
}

export function getComments(postId) {
  return request(`/posts/${postId}/comments`);
}
