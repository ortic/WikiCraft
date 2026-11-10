# Inputs that must stay inert

[js](javascript:alert(1)) and [data](data:text/html,boom) links.

<script>alert('x')</script>

<img src=x onerror=alert(1)>

<a href="https://ok.example" onclick="steal()">html link</a>

![bad](javascript:alert(2))

Text with <b>inline html</b> and an entity &copy; and a literal \*star\*.
